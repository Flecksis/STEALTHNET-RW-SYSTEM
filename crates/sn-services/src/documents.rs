use crate::Actor;
use crate::bot_content::{CONSENT_KEY, DOCUMENTS_KEY, Document};
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sn_domain::client::Locale;
use sn_domain::documents::{self as rules, BODY_MAX};
use sn_domain::ids::AdminId;
use sn_store::{Db, DocumentPage, LockSpace};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub const DOCUMENTS_MAX: usize = 10;
pub const DOCUMENT_TITLE_MAX: usize = 80;
pub const DOCUMENT_URL_MAX: usize = 500;
const ID_LEN: usize = 10;
const CODE: &str = "bot_documents.invalid";

type Bodies = (String, Option<String>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    Link,
    Page,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocumentRow {
    pub id: String,
    pub kind: DocumentKind,
    pub title: String,
    pub title_en: Option<String>,
    pub url: Option<String>,
    pub path: Option<String>,
    pub english: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocumentsView {
    pub consent: bool,
    pub items: Vec<DocumentRow>,
    pub max: usize,
    pub title_max: usize,
    pub url_max: usize,
    pub path_max: usize,
    pub body_max: usize,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ListItem {
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub title_en: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ListUpdate {
    pub consent: bool,
    #[serde(default)]
    pub items: Vec<ListItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DocumentInput {
    pub kind: DocumentKind,
    pub title: String,
    #[serde(default)]
    pub title_en: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub body_ru: Option<String>,
    #[serde(default)]
    pub body_en: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocumentFull {
    pub id: String,
    pub kind: DocumentKind,
    pub title: String,
    pub title_en: Option<String>,
    pub url: Option<String>,
    pub path: Option<String>,
    pub link: Option<String>,
    pub body_ru: Option<String>,
    pub body_en: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CleanView {
    pub html: String,
    pub chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicDocument {
    pub title: String,
    pub title_en: Option<String>,
    pub url: String,
    pub page: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicPage {
    pub path: String,
    pub title: String,
    pub lang: Locale,
    pub html: String,
    pub description: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub english: bool,
}

fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::rejected(CODE, message)
}

fn one_line(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clean_title(raw: &str) -> ServiceResult<String> {
    let title = one_line(raw);
    if title.is_empty() {
        return Err(invalid("У документа нет названия"));
    }
    if title.chars().count() > DOCUMENT_TITLE_MAX {
        return Err(invalid(format!(
            "Название «{title}» длиннее {DOCUMENT_TITLE_MAX} символов"
        )));
    }
    Ok(title)
}

fn clean_title_en(raw: Option<&str>) -> ServiceResult<Option<String>> {
    let Some(title) = raw.map(one_line).filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    if title.chars().count() > DOCUMENT_TITLE_MAX {
        return Err(invalid(format!(
            "Английское название «{title}» длиннее {DOCUMENT_TITLE_MAX} символов"
        )));
    }
    Ok(Some(title))
}

fn clean_url(title: &str, raw: Option<&str>) -> ServiceResult<String> {
    let url = raw.unwrap_or("").trim().to_string();
    let well_formed = url.starts_with("https://")
        && url.len() > "https://".len()
        && url.len() <= DOCUMENT_URL_MAX
        && !url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '<' | '>' | '\''));
    if !well_formed {
        return Err(invalid(format!(
            "У документа «{title}» нужна ссылка вида https://…"
        )));
    }
    Ok(url)
}

pub fn clean_path(raw: &str, web_dir: Option<&Path>) -> ServiceResult<String> {
    let path = rules::clean_path(raw).map_err(|problem| invalid(problem.to_string()))?;
    let first = rules::first_segment(&path);
    if let Some(dir) = web_dir
        && dir.join(first).exists()
    {
        return Err(invalid(format!(
            "Адрес /{first} занят файлами сайта. Выберите другой"
        )));
    }
    Ok(path)
}

fn clean_body(raw: &str, language: &str) -> ServiceResult<Option<String>> {
    let html = rules::clean_html(raw);
    if rules::text_len(&html) == 0 {
        return Ok(None);
    }
    if html.len() > BODY_MAX {
        return Err(invalid(format!(
            "{language} текст страницы слишком длинный: после очистки {} КБ, можно до {} КБ",
            html.len().div_ceil(1000),
            BODY_MAX / 1000
        )));
    }
    Ok(Some(html))
}

fn new_id(taken: &HashSet<&str>) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    loop {
        let id: String = (0..ID_LEN)
            .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
            .collect();
        if !taken.contains(id.as_str()) {
            return id;
        }
    }
}

fn unique_targets(items: &[Document]) -> ServiceResult<()> {
    let mut seen: HashMap<&str, &str> = HashMap::new();
    for item in items {
        let target = item.target();
        if let Some(other) = seen.insert(target, &item.title) {
            return Err(invalid(match &item.path {
                Some(path) => format!("Адрес {path} уже у документа «{other}»"),
                None => format!("Ссылка {target} уже у документа «{other}»"),
            }));
        }
    }
    Ok(())
}

fn kind_of(document: &Document) -> DocumentKind {
    if document.is_page() {
        DocumentKind::Page
    } else {
        DocumentKind::Link
    }
}

fn full(document: &Document, page: Option<DocumentPage>, base: Option<&str>) -> DocumentFull {
    let (body_ru, body_en, updated_at) = match page {
        Some(page) => (Some(page.body_ru), page.body_en, Some(page.updated_at)),
        None => (None, None, None),
    };
    DocumentFull {
        id: document.id.clone(),
        kind: kind_of(document),
        title: document.title.clone(),
        title_en: document.title_en.clone(),
        url: (!document.is_page()).then(|| document.url.clone()),
        path: document.path.clone(),
        link: document.absolute(base),
        body_ru,
        body_en,
        updated_at,
    }
}

pub fn public_list(documents: &[Document]) -> Vec<PublicDocument> {
    documents
        .iter()
        .map(|d| PublicDocument {
            title: d.title.clone(),
            title_en: d.title_en.clone(),
            url: d.target().to_string(),
            page: d.is_page(),
        })
        .collect()
}

pub fn find_page<'d>(documents: &'d [Document], path: &str) -> Option<&'d Document> {
    documents.iter().find(|d| d.path.as_deref() == Some(path))
}

pub struct DocumentOps<'a> {
    services: &'a Services,
    web_dir: Option<&'a Path>,
}

impl<'a> DocumentOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        DocumentOps {
            services,
            web_dir: None,
        }
    }

    pub fn web_dir(mut self, dir: Option<&'a Path>) -> Self {
        self.web_dir = dir;
        self
    }

    fn base_url(&self) -> Option<String> {
        let settings = self.services.settings();
        let base = settings.public_url.as_deref()?.trim().trim_end_matches('/');
        (base.starts_with("https://") || base.starts_with("http://")).then(|| base.to_string())
    }

    pub async fn view(&self) -> ServiceResult<DocumentsView> {
        let runtime = self.services.bot_content();
        let infos: HashMap<String, (bool, DateTime<Utc>)> = {
            let mut conn = self.services.store().conn().await?;
            conn.document_pages()
                .infos()
                .await?
                .into_iter()
                .map(|info| (info.id, (info.english, info.updated_at)))
                .collect()
        };
        Ok(DocumentsView {
            consent: runtime.consent,
            items: runtime
                .documents
                .iter()
                .map(|d| {
                    let info = infos.get(&d.id);
                    DocumentRow {
                        id: d.id.clone(),
                        kind: kind_of(d),
                        title: d.title.clone(),
                        title_en: d.title_en.clone(),
                        url: (!d.is_page()).then(|| d.url.clone()),
                        path: d.path.clone(),
                        english: d.is_page() && info.is_some_and(|i| i.0),
                        updated_at: info.map(|i| i.1),
                    }
                })
                .collect(),
            max: DOCUMENTS_MAX,
            title_max: DOCUMENT_TITLE_MAX,
            url_max: DOCUMENT_URL_MAX,
            path_max: rules::PATH_MAX,
            body_max: BODY_MAX,
            base_url: self.base_url(),
        })
    }

    pub async fn get(&self, id: &str) -> ServiceResult<DocumentFull> {
        let runtime = self.services.bot_content();
        let document = runtime
            .documents
            .iter()
            .find(|d| d.id == id)
            .ok_or(ServiceError::NotFound("документ"))?;
        let page = if document.is_page() {
            let mut conn = self.services.store().conn().await?;
            conn.document_pages().get(id).await?
        } else {
            None
        };
        Ok(full(document, page, self.base_url().as_deref()))
    }

    pub fn clean(&self, raw: &str) -> CleanView {
        let html = rules::clean_html(raw);
        let chars = rules::text_len(&html);
        CleanView { html, chars }
    }

    async fn stored(tx: &mut sn_store::Tx) -> ServiceResult<(Vec<Document>, bool)> {
        let documents: Vec<Document> = tx
            .settings()
            .get(DOCUMENTS_KEY)
            .await?
            .and_then(|v| serde_json::from_value::<Vec<Document>>(v).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(Document::valid)
            .collect();
        let consent = tx
            .settings()
            .get(CONSENT_KEY)
            .await?
            .and_then(|v| serde_json::from_value::<bool>(v).ok())
            .unwrap_or(true);
        Ok((crate::bot_content::with_ids(documents), consent))
    }

    async fn write(
        &self,
        tx: &mut sn_store::Tx,
        documents: &[Document],
        consent: bool,
        admin: Option<AdminId>,
    ) -> ServiceResult<()> {
        tx.settings()
            .set_as(DOCUMENTS_KEY, &documents, admin)
            .await?;
        tx.settings().set_as(CONSENT_KEY, &consent, admin).await?;
        let pages: Vec<String> = documents
            .iter()
            .filter(|d| d.is_page())
            .map(|d| d.id.clone())
            .collect();
        tx.document_pages().keep_only(&pages).await?;
        Ok(())
    }

    async fn audit(
        tx: &mut sn_store::Tx,
        admin: AdminId,
        details: serde_json::Value,
    ) -> ServiceResult<()> {
        tx.audit()
            .record(&Actor::Admin(admin).audit("settings.documents", "settings", None, details))
            .await?;
        Ok(())
    }

    async fn saved(&self) -> ServiceResult<()> {
        self.services.reload().await?;
        self.services.notify_settings_changed().await
    }

    pub async fn replace(
        &self,
        update: &ListUpdate,
        admin: Option<AdminId>,
    ) -> ServiceResult<DocumentsView> {
        if update.items.len() > DOCUMENTS_MAX {
            return Err(invalid(format!(
                "Документов может быть не больше {DOCUMENTS_MAX}"
            )));
        }
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::Documents, 0).await?;
        let (current, current_consent) = Self::stored(&mut tx).await?;
        let mut taken: HashSet<String> = current.iter().map(|d| d.id.clone()).collect();
        let mut seen_ids: HashSet<String> = HashSet::new();
        let mut next = Vec::with_capacity(update.items.len());
        for item in &update.items {
            let title = clean_title(&item.title)?;
            let title_en = clean_title_en(item.title_en.as_deref())?;
            let known = item
                .id
                .as_deref()
                .and_then(|id| current.iter().find(|d| d.id == id));
            let id = match known {
                Some(document) => document.id.clone(),
                None => {
                    let id = new_id(&taken.iter().map(String::as_str).collect());
                    taken.insert(id.clone());
                    id
                }
            };
            if !seen_ids.insert(id.clone()) {
                return Err(invalid(format!(
                    "Документ «{title}» встречается в списке дважды"
                )));
            }
            if known.is_some_and(Document::is_page) || item.path.is_some() {
                let Some(existing) = known.filter(|d| d.is_page()) else {
                    return Err(invalid(format!(
                        "Страницу «{title}» нужно сначала создать: «Новая страница» в списке документов"
                    )));
                };
                let raw = item
                    .path
                    .as_deref()
                    .or(existing.path.as_deref())
                    .unwrap_or("");
                next.push(Document {
                    id,
                    title,
                    title_en,
                    url: String::new(),
                    path: Some(clean_path(raw, self.web_dir)?),
                });
            } else {
                let url = clean_url(&title, item.url.as_deref())?;
                next.push(Document {
                    id,
                    title,
                    title_en,
                    url,
                    path: None,
                });
            }
        }
        unique_targets(&next)?;
        if next == current && update.consent == current_consent {
            tx.rollback().await?;
            return self.view().await;
        }
        self.write(&mut tx, &next, update.consent, admin).await?;
        if let Some(admin) = admin {
            Self::audit(
                &mut tx,
                admin,
                json!({
                    "action": "list",
                    "count": next.len(),
                    "titles": next.iter().map(|d| d.title.as_str()).collect::<Vec<_>>(),
                    "consent": update.consent,
                }),
            )
            .await?;
        }
        tx.commit().await?;
        self.saved().await?;
        self.view().await
    }

    fn document_from(
        &self,
        id: String,
        input: &DocumentInput,
    ) -> ServiceResult<(Document, Option<Bodies>)> {
        let title = clean_title(&input.title)?;
        let title_en = clean_title_en(input.title_en.as_deref())?;
        match input.kind {
            DocumentKind::Link => {
                let url = clean_url(&title, input.url.as_deref())?;
                Ok((
                    Document {
                        id,
                        title,
                        title_en,
                        url,
                        path: None,
                    },
                    None,
                ))
            }
            DocumentKind::Page => {
                let path = clean_path(input.path.as_deref().unwrap_or(""), self.web_dir)?;
                let Some(body_ru) = clean_body(input.body_ru.as_deref().unwrap_or(""), "Русский")?
                else {
                    return Err(invalid(format!(
                        "Добавьте текст страницы «{title}» на русском"
                    )));
                };
                let body_en = match input.body_en.as_deref() {
                    Some(raw) => clean_body(raw, "Английский")?,
                    None => None,
                };
                Ok((
                    Document {
                        id,
                        title,
                        title_en,
                        url: String::new(),
                        path: Some(path),
                    },
                    Some((body_ru, body_en)),
                ))
            }
        }
    }

    pub async fn create(
        &self,
        input: &DocumentInput,
        admin: AdminId,
    ) -> ServiceResult<DocumentFull> {
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::Documents, 0).await?;
        let (mut documents, consent) = Self::stored(&mut tx).await?;
        if documents.len() >= DOCUMENTS_MAX {
            return Err(invalid(format!(
                "Документов может быть не больше {DOCUMENTS_MAX}"
            )));
        }
        let id = new_id(&documents.iter().map(|d| d.id.as_str()).collect());
        let (document, bodies) = self.document_from(id, input)?;
        documents.push(document.clone());
        unique_targets(&documents)?;
        self.write(&mut tx, &documents, consent, Some(admin))
            .await?;
        let page = match &bodies {
            Some((ru, en)) => Some(
                tx.document_pages()
                    .put(&document.id, ru, en.as_deref(), Some(admin))
                    .await?,
            ),
            None => None,
        };
        Self::audit(
            &mut tx,
            admin,
            json!({
                "action": "create",
                "id": document.id,
                "title": document.title,
                "kind": kind_of(&document),
                "target": document.target(),
            }),
        )
        .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(full(&document, page, self.base_url().as_deref()))
    }

    pub async fn update(
        &self,
        id: &str,
        input: &DocumentInput,
        admin: AdminId,
    ) -> ServiceResult<DocumentFull> {
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::Documents, 0).await?;
        let (mut documents, consent) = Self::stored(&mut tx).await?;
        let Some(index) = documents.iter().position(|d| d.id == id) else {
            return Err(ServiceError::NotFound("документ"));
        };
        let (document, bodies) = self.document_from(id.to_string(), input)?;
        let before = documents[index].clone();
        documents[index] = document.clone();
        unique_targets(&documents)?;
        self.write(&mut tx, &documents, consent, Some(admin))
            .await?;
        let page = match &bodies {
            Some((ru, en)) => Some(
                tx.document_pages()
                    .put(&document.id, ru, en.as_deref(), Some(admin))
                    .await?,
            ),
            None => None,
        };
        Self::audit(
            &mut tx,
            admin,
            json!({
                "action": "update",
                "id": document.id,
                "title": document.title,
                "kind": kind_of(&document),
                "target": document.target(),
                "was": {"title": before.title, "target": before.target()},
            }),
        )
        .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(full(&document, page, self.base_url().as_deref()))
    }

    pub async fn remove(&self, id: &str, admin: AdminId) -> ServiceResult<DocumentsView> {
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::Documents, 0).await?;
        let (mut documents, consent) = Self::stored(&mut tx).await?;
        let Some(index) = documents.iter().position(|d| d.id == id) else {
            return Err(ServiceError::NotFound("документ"));
        };
        let removed = documents.remove(index);
        self.write(&mut tx, &documents, consent, Some(admin))
            .await?;
        Self::audit(
            &mut tx,
            admin,
            json!({
                "action": "remove",
                "id": removed.id,
                "title": removed.title,
                "kind": kind_of(&removed),
                "target": removed.target(),
            }),
        )
        .await?;
        tx.commit().await?;
        self.saved().await?;
        self.view().await
    }

    pub async fn page(&self, path: &str, wanted: Locale) -> ServiceResult<Option<PublicPage>> {
        let runtime = self.services.bot_content();
        let Some(document) = find_page(&runtime.documents, path) else {
            return Ok(None);
        };
        let stored = {
            let mut conn = self.services.store().conn().await?;
            conn.document_pages().get(&document.id).await?
        };
        let Some(stored) = stored else {
            return Ok(None);
        };
        let english = stored.body_en.is_some();
        let (lang, raw) = match (wanted, stored.body_en) {
            (Locale::En, Some(en)) => (Locale::En, en),
            _ => (Locale::Ru, stored.body_ru),
        };
        let html = rules::clean_html(&raw);
        Ok(Some(PublicPage {
            path: path.to_string(),
            title: document.title_in(lang).to_string(),
            lang,
            description: rules::description(&html),
            html,
            updated_at: stored.updated_at,
            english,
        }))
    }
}
