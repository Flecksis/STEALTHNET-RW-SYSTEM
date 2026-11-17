use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sn_domain::ids::AdminId;
use sn_domain::image::{self, ImageKind, ImageProblem};
use sn_domain::settings::{LogoMeta, SITE_DESCRIPTION_MAX, SITE_TITLE_MAX, keys};
use sn_store::{Db, SiteAsset};

pub const LOGO_ASSET: &str = "logo";
const TAG_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogoView {
    pub url: String,
    pub content_type: &'static str,
    pub width: u32,
    pub height: u32,
    pub size: u32,
}

impl LogoView {
    pub fn of(meta: &LogoMeta) -> Self {
        LogoView {
            url: meta.path(),
            content_type: meta.kind.content_type(),
            width: meta.width,
            height: meta.height,
            size: meta.size,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SiteView {
    pub title: Option<String>,
    pub description: Option<String>,
    pub indexing: bool,
    pub brand_name: String,
    pub public_url: Option<String>,
    pub logo: Option<LogoView>,
    pub title_max: usize,
    pub description_max: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SiteUpdate {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub indexing: Option<bool>,
}

pub fn brand_initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "S".to_string())
}

pub fn clean_text(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

fn field_error(field: &str, reason: &str) -> ServiceError {
    ServiceError::rejected(
        "settings.invalid_field",
        format!("поле «{field}»: {reason}"),
    )
}

fn problem(problem: ImageProblem) -> ServiceError {
    match problem {
        ImageProblem::Empty => ServiceError::rejected("logo.empty", "Файл пустой"),
        ImageProblem::TooHeavy => {
            ServiceError::rejected("logo.too_heavy", "Файл больше 1 МБ: уменьшите картинку")
        }
        ImageProblem::Format => {
            ServiceError::rejected("logo.format", "Подходят только PNG, JPEG и WebP")
        }
        ImageProblem::Damaged => ServiceError::rejected(
            "logo.damaged",
            "Картинка повреждена или сохранена не до конца",
        ),
        ImageProblem::TooSmall => ServiceError::rejected(
            "logo.too_small",
            format!("Картинка меньше {0}×{0} точек", image::LOGO_MIN_SIDE),
        ),
        ImageProblem::TooLarge => ServiceError::rejected(
            "logo.too_large",
            format!("Картинка больше {} точек по стороне", image::LOGO_MAX_SIDE),
        ),
    }
}

pub struct BrandingOps<'a> {
    services: &'a Services,
}

impl<'a> BrandingOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        BrandingOps { services }
    }

    async fn saved(&self) -> ServiceResult<()> {
        self.services.reload().await?;
        self.services.notify_settings_changed().await
    }

    pub fn site(&self) -> SiteView {
        let settings = self.services.settings();
        SiteView {
            title: settings.site.title.clone(),
            description: settings.site.description.clone(),
            indexing: settings.site.indexing,
            brand_name: settings.brand_name.clone(),
            public_url: settings.public_url.clone(),
            logo: settings.logo.as_ref().map(LogoView::of),
            title_max: SITE_TITLE_MAX,
            description_max: SITE_DESCRIPTION_MAX,
        }
    }

    pub async fn update_site(
        &self,
        update: &SiteUpdate,
        admin: AdminId,
    ) -> ServiceResult<SiteView> {
        let current = self.services.settings().site.clone();
        let mut next = current.clone();
        if let Some(title) = &update.title {
            let title = clean_text(title);
            if title.chars().count() > SITE_TITLE_MAX {
                return Err(field_error(
                    "title",
                    &format!("не длиннее {SITE_TITLE_MAX} символов"),
                ));
            }
            next.title = (!title.is_empty()).then_some(title);
        }
        if let Some(description) = &update.description {
            let description = clean_text(description);
            if description.chars().count() > SITE_DESCRIPTION_MAX {
                return Err(field_error(
                    "description",
                    &format!("не длиннее {SITE_DESCRIPTION_MAX} символов"),
                ));
            }
            next.description = (!description.is_empty()).then_some(description);
        }
        if let Some(indexing) = update.indexing {
            next.indexing = indexing;
        }
        if next == current {
            return Ok(self.site());
        }
        let mut tx = self.services.store().begin().await?;
        tx.settings().set_as(keys::SITE, &next, Some(admin)).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.site",
                "settings",
                None,
                json!({
                    "title": next.title != current.title,
                    "description": next.description != current.description,
                    "indexing": next.indexing,
                }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.site())
    }

    pub async fn upload_logo(&self, bytes: &[u8], admin: AdminId) -> ServiceResult<LogoView> {
        let info = image::check_logo(bytes).map_err(problem)?;
        let digest = hex::encode(Sha256::digest(bytes));
        let meta = LogoMeta {
            tag: digest[..TAG_LEN].to_string(),
            kind: info.kind,
            width: info.width,
            height: info.height,
            size: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
        };
        let asset = SiteAsset {
            content_type: info.kind.content_type().to_string(),
            bytes: bytes.to_vec(),
            sha256: digest.clone(),
        };
        let mut tx = self.services.store().begin().await?;
        tx.site_assets()
            .put(LOGO_ASSET, &asset, Some(admin))
            .await?;
        tx.settings().set_as(keys::LOGO, &meta, Some(admin)).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.logo",
                "settings",
                None,
                json!({
                    "action": "uploaded",
                    "kind": info.kind,
                    "width": info.width,
                    "height": info.height,
                    "size": meta.size,
                    "sha256": digest,
                }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(LogoView::of(&meta))
    }

    pub async fn delete_logo(&self, admin: AdminId) -> ServiceResult<bool> {
        let mut tx = self.services.store().begin().await?;
        let removed = tx.site_assets().delete(LOGO_ASSET).await?;
        let cleared = tx.settings().delete(keys::LOGO).await?;
        if !(removed || cleared) {
            tx.commit().await?;
            return Ok(false);
        }
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.logo",
                "settings",
                None,
                json!({ "action": "removed" }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(true)
    }

    pub async fn logo(&self, tag: &str) -> ServiceResult<Option<(ImageKind, Vec<u8>)>> {
        let Some(meta) = self.services.settings().logo.clone() else {
            return Ok(None);
        };
        if meta.tag != tag {
            return Ok(None);
        }
        let mut conn = self.services.store().conn().await?;
        let asset = conn.site_assets().get(LOGO_ASSET).await?;
        Ok(asset
            .filter(|asset| asset.sha256.starts_with(&meta.tag))
            .map(|asset| (meta.kind, asset.bytes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn знак_без_логотипа_берёт_первую_букву_названия() {
        assert_eq!(brand_initial("stealthnet"), "S");
        assert_eq!(brand_initial("  мой впн"), "М");
        assert_eq!(brand_initial("«Тихий» VPN"), "Т");
        assert_eq!(brand_initial("—"), "S");
    }

    #[test]
    fn тексты_сайта_без_переносов_и_служебных_символов() {
        assert_eq!(clean_text("  VPN\n\tдля\u{0} всех  "), "VPN для всех");
        assert_eq!(clean_text("   "), "");
    }
}
