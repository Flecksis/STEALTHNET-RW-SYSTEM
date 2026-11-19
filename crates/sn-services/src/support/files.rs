use super::SupportOps;
use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use chrono::{DateTime, Utc};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::Serialize;
use sn_domain::ids::TicketFileId;
use sn_domain::module::ModuleId;
use sn_domain::support::{FileKind, TicketFile};
use sn_modules::catalog::ticket_files::{self, Limits, Refusal};
use sn_store::{Db, FileOwner, NewTicketFile, StoredFile};

pub const LINK_MINUTES: i64 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDownload {
    pub name: String,
    pub mime: String,
    pub kind: FileKind,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileLink {
    pub token: String,
    pub name: String,
    pub until: DateTime<Utc>,
}

fn files_off() -> ServiceError {
    ServiceError::rejected(
        "support.files_off",
        "Файлы в обращениях выключены: опишите вопрос текстом",
    )
}

pub fn type_refused() -> ServiceError {
    ServiceError::rejected(
        "support.file_type",
        format!(
            "Такой файл не подойдёт. Можно прикрепить {}",
            ticket_files::ALLOWED_TEXT
        ),
    )
}

pub fn too_big(limits: &Limits) -> ServiceError {
    ServiceError::rejected(
        "support.file_big",
        format!("Файл больше {} МБ", limits.max_mb()),
    )
}

fn no_room(text: &str) -> ServiceError {
    ServiceError::rejected("support.files_quota", text)
}

const fn owner_of(actor: Actor) -> Option<FileOwner> {
    match actor {
        Actor::Client(id) => Some(FileOwner::Client(id)),
        Actor::Admin(id) => Some(FileOwner::Admin(id)),
        Actor::System => None,
    }
}

fn visible_to(stored: &StoredFile, actor: Actor) -> bool {
    match (actor, stored.ticket_id) {
        (Actor::System, _) => true,
        (Actor::Client(client), Some(_)) => stored.ticket_client == Some(client),
        (Actor::Admin(_), Some(_)) => true,
        (actor, None) => owner_of(actor).is_some() && stored.owner == owner_of(actor),
    }
}

fn token() -> String {
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

fn download(stored: StoredFile) -> ServiceResult<FileDownload> {
    let bytes = stored.bytes.ok_or_else(|| {
        ServiceError::rejected("support.file_removed", "Файл удалён по сроку хранения")
    })?;
    Ok(FileDownload {
        name: stored.file.name,
        mime: stored.file.mime,
        kind: stored.file.kind,
        bytes,
    })
}

impl SupportOps<'_> {
    pub fn file_limits(&self) -> Option<Limits> {
        let modules = self.services.modules();
        modules
            .is_enabled(ModuleId::TicketFiles)
            .then(|| Limits::of(&modules.config(ModuleId::TicketFiles)))
    }

    pub(super) async fn checked_files(
        &self,
        db: &mut impl Db,
        actor: Actor,
        files: &[TicketFileId],
    ) -> ServiceResult<Vec<TicketFile>> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let limits = self.file_limits().ok_or_else(files_off)?;
        let owner = owner_of(actor).ok_or_else(files_off)?;
        let mut ids = files.to_vec();
        ids.sort_unstable();
        ids.dedup();
        if ids.len() > limits.max_files {
            return Err(ServiceError::rejected(
                "support.files_many",
                format!(
                    "Слишком много файлов: в одном сообщении не больше {}",
                    limits.max_files
                ),
            ));
        }
        let found = db.ticket_files().pending(owner, &ids).await?;
        if found.len() != ids.len() {
            return Err(ServiceError::rejected(
                "support.file_missing",
                "Файл не найден: прикрепите его ещё раз",
            ));
        }
        Ok(found)
    }

    pub async fn upload(
        &self,
        actor: Actor,
        name: &str,
        bytes: Vec<u8>,
    ) -> ServiceResult<TicketFile> {
        let limits = self.file_limits().ok_or_else(files_off)?;
        let owner = owner_of(actor).ok_or_else(files_off)?;
        if i64::try_from(bytes.len()).unwrap_or(i64::MAX) > limits.max_bytes {
            return Err(too_big(&limits));
        }
        let file_type = ticket_files::detect(name, &bytes).map_err(|refusal| match refusal {
            Refusal::Empty => ServiceError::rejected("support.file_empty", "Файл пустой"),
            Refusal::Type => type_refused(),
        })?;
        let cleaned = ticket_files::clean(file_type, bytes).ok_or_else(|| {
            ServiceError::rejected(
                "support.file_broken",
                "Файл повреждён: его не получилось открыть",
            )
        })?;
        let name = ticket_files::file_name(name, file_type);
        let mut conn = self.services.store().conn().await?;
        if let FileOwner::Client(client_id) = owner {
            if conn.ticket_files().uploaded_today(client_id).await? >= limits.daily_files() {
                return Err(ServiceError::rejected(
                    "support.files_daily",
                    "Сегодня отправлено слишком много файлов, попробуйте завтра",
                ));
            }
            let size = i64::try_from(cleaned.bytes.len()).unwrap_or(i64::MAX);
            if conn
                .ticket_files()
                .client_bytes(client_id)
                .await?
                .saturating_add(size)
                > limits.client_bytes
            {
                return Err(no_room(
                    "Место для ваших файлов закончилось: опишите вопрос текстом",
                ));
            }
            if conn
                .ticket_files()
                .stored_bytes()
                .await?
                .saturating_add(size)
                > limits.total_bytes
            {
                return Err(no_room(
                    "Место для файлов в поддержке закончилось: опишите вопрос текстом",
                ));
            }
        }
        Ok(conn
            .ticket_files()
            .insert(&NewTicketFile {
                owner,
                name: &name,
                mime: file_type.mime,
                bytes: &cleaned.bytes,
                width: cleaned.width,
                height: cleaned.height,
            })
            .await?)
    }

    pub async fn discard_file(&self, id: TicketFileId, actor: Actor) -> ServiceResult<()> {
        let owner = owner_of(actor).ok_or(ServiceError::NotFound("файл"))?;
        let mut conn = self.services.store().conn().await?;
        if !conn.ticket_files().discard(id, owner).await? {
            return Err(ServiceError::NotFound("файл"));
        }
        Ok(())
    }

    async fn visible(
        &self,
        db: &mut impl Db,
        id: TicketFileId,
        actor: Actor,
    ) -> ServiceResult<StoredFile> {
        db.ticket_files()
            .content(id)
            .await?
            .filter(|stored| visible_to(stored, actor))
            .ok_or(ServiceError::NotFound("файл"))
    }

    pub async fn file(&self, id: TicketFileId, actor: Actor) -> ServiceResult<FileDownload> {
        let mut conn = self.services.store().conn().await?;
        download(self.visible(&mut conn, id, actor).await?)
    }

    pub async fn file_link(&self, id: TicketFileId, actor: Actor) -> ServiceResult<FileLink> {
        let mut conn = self.services.store().conn().await?;
        let stored = self.visible(&mut conn, id, actor).await?;
        if stored.bytes.is_none() || stored.ticket_id.is_none() {
            return Err(ServiceError::rejected(
                "support.file_removed",
                "Файл удалён по сроку хранения",
            ));
        }
        let token = token();
        let until = conn
            .ticket_files()
            .issue_link(id, &token, LINK_MINUTES)
            .await?;
        Ok(FileLink {
            token,
            name: stored.file.name,
            until,
        })
    }

    pub async fn file_by_link(&self, token: &str) -> ServiceResult<FileDownload> {
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ServiceError::NotFound("файл"));
        }
        let mut conn = self.services.store().conn().await?;
        download(
            conn.ticket_files()
                .by_link(token)
                .await?
                .ok_or(ServiceError::NotFound("файл"))?,
        )
    }
}
