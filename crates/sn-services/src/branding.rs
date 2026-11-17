use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sn_domain::ids::AdminId;
use sn_domain::image::{self, ImageKind, ImageProblem};
use sn_domain::settings::{
    CoreSettings, ImageMeta, SITE_DESCRIPTION_MAX, SITE_TITLE_MAX, SiteImage, keys,
};
use sn_store::{Db, SiteAsset};

const TAG_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImageView {
    pub url: String,
    pub content_type: &'static str,
    pub width: u32,
    pub height: u32,
    pub size: u32,
}

impl ImageView {
    pub fn of(meta: &ImageMeta, image: SiteImage) -> Self {
        ImageView {
            url: meta.path(image),
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
    pub logo: Option<ImageView>,
    pub favicon: Option<ImageView>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabIcon<'a> {
    Image(SiteImage, &'a ImageMeta),
    Mark,
}

pub fn tab_icon(settings: &CoreSettings) -> TabIcon<'_> {
    if let Some(favicon) = &settings.favicon {
        TabIcon::Image(SiteImage::Favicon, favicon)
    } else if let Some(logo) = &settings.logo {
        TabIcon::Image(SiteImage::Logo, logo)
    } else {
        TabIcon::Mark
    }
}

pub fn touch_icon(settings: &CoreSettings) -> Option<String> {
    let usable = |meta: &&ImageMeta| matches!(meta.kind, ImageKind::Png | ImageKind::Jpeg);
    settings
        .favicon
        .as_ref()
        .filter(usable)
        .map(|meta| meta.path(SiteImage::Favicon))
        .or_else(|| {
            settings
                .logo
                .as_ref()
                .filter(usable)
                .map(|meta| meta.path(SiteImage::Logo))
        })
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

fn problem(image: SiteImage, problem: ImageProblem) -> ServiceError {
    let rules = image.rules();
    let (logo, favicon, message) = match problem {
        ImageProblem::Empty => ("logo.empty", "favicon.empty", "Файл пустой".to_string()),
        ImageProblem::TooHeavy => (
            "logo.too_heavy",
            "favicon.too_heavy",
            match image {
                SiteImage::Logo => "Файл больше 1 МБ: уменьшите картинку".to_string(),
                SiteImage::Favicon => "Файл больше 512 КБ: уменьшите значок".to_string(),
            },
        ),
        ImageProblem::Format => (
            "logo.format",
            "favicon.format",
            match image {
                SiteImage::Logo => "Подходят только PNG, JPEG и WebP".to_string(),
                SiteImage::Favicon => "Подходят только PNG и ICO".to_string(),
            },
        ),
        ImageProblem::Damaged => (
            "logo.damaged",
            "favicon.damaged",
            "Картинка повреждена или сохранена не до конца".to_string(),
        ),
        ImageProblem::TooSmall => (
            "logo.too_small",
            "favicon.too_small",
            format!("Картинка меньше {0}×{0} точек", rules.min_side),
        ),
        ImageProblem::TooLarge => (
            "logo.too_large",
            "favicon.too_large",
            format!("Картинка больше {} точек по стороне", rules.max_side),
        ),
        ImageProblem::NotSquare => (
            "logo.not_square",
            "favicon.not_square",
            "Значок должен быть квадратным: ширина равна высоте".to_string(),
        ),
    };
    let code = match image {
        SiteImage::Logo => logo,
        SiteImage::Favicon => favicon,
    };
    ServiceError::rejected(code, message)
}

fn audit_action(image: SiteImage) -> &'static str {
    match image {
        SiteImage::Logo => "settings.logo",
        SiteImage::Favicon => "settings.favicon",
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
            logo: settings
                .logo
                .as_ref()
                .map(|meta| ImageView::of(meta, SiteImage::Logo)),
            favicon: settings
                .favicon
                .as_ref()
                .map(|meta| ImageView::of(meta, SiteImage::Favicon)),
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

    pub async fn upload(
        &self,
        image: SiteImage,
        bytes: &[u8],
        admin: AdminId,
    ) -> ServiceResult<ImageView> {
        let info = image::check(bytes, image.rules()).map_err(|reason| problem(image, reason))?;
        let digest = hex::encode(Sha256::digest(bytes));
        let meta = ImageMeta {
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
            .put(image.name(), &asset, Some(admin))
            .await?;
        tx.settings()
            .set_as(image.key(), &meta, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                audit_action(image),
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
        Ok(ImageView::of(&meta, image))
    }

    pub async fn remove(&self, image: SiteImage, admin: AdminId) -> ServiceResult<bool> {
        let mut tx = self.services.store().begin().await?;
        let removed = tx.site_assets().delete(image.name()).await?;
        let cleared = tx.settings().delete(image.key()).await?;
        if !(removed || cleared) {
            tx.commit().await?;
            return Ok(false);
        }
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                audit_action(image),
                "settings",
                None,
                json!({ "action": "removed" }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(true)
    }

    pub async fn file(
        &self,
        image: SiteImage,
        tag: &str,
    ) -> ServiceResult<Option<(ImageKind, Vec<u8>)>> {
        let Some(meta) = self.services.settings().image(image).cloned() else {
            return Ok(None);
        };
        if meta.tag != tag {
            return Ok(None);
        }
        let mut conn = self.services.store().conn().await?;
        let asset = conn.site_assets().get(image.name()).await?;
        Ok(asset
            .filter(|asset| asset.sha256.starts_with(&meta.tag))
            .map(|asset| (meta.kind, asset.bytes)))
    }

    pub async fn tab_icon_file(&self) -> ServiceResult<Option<(ImageKind, Vec<u8>)>> {
        let settings = self.services.settings();
        match tab_icon(&settings) {
            TabIcon::Image(image, meta) => self.file(image, &meta.tag).await,
            TabIcon::Mark => Ok(None),
        }
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

    fn meta(kind: ImageKind, tag: &str) -> ImageMeta {
        ImageMeta {
            tag: tag.into(),
            kind,
            width: 256,
            height: 256,
            size: 1000,
        }
    }

    #[test]
    fn значок_вкладки_фавикон_потом_логотип_потом_знак() {
        let mut settings = CoreSettings::default();
        assert_eq!(tab_icon(&settings), TabIcon::Mark);
        assert_eq!(touch_icon(&settings), None);
        settings.logo = Some(meta(ImageKind::Webp, "aaaa"));
        assert!(matches!(
            tab_icon(&settings),
            TabIcon::Image(SiteImage::Logo, _)
        ));
        assert_eq!(touch_icon(&settings), None);
        settings.logo = Some(meta(ImageKind::Png, "bbbb"));
        assert_eq!(
            touch_icon(&settings).as_deref(),
            Some("/api/public/logo/bbbb.png")
        );
        settings.favicon = Some(meta(ImageKind::Ico, "cccc"));
        assert!(matches!(
            tab_icon(&settings),
            TabIcon::Image(SiteImage::Favicon, _)
        ));
        assert_eq!(
            touch_icon(&settings).as_deref(),
            Some("/api/public/logo/bbbb.png")
        );
        settings.favicon = Some(meta(ImageKind::Png, "dddd"));
        assert_eq!(
            touch_icon(&settings).as_deref(),
            Some("/api/public/favicon/dddd.png")
        );
    }
}
