use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sn_domain::connect::{
    self, APPS_MAX, AppIcon, BUTTONS_MAX, ConnectApp, ConnectApps, ICON_RULES, ICON_SVG_MAX,
    ICON_TAG_LEN, IconKind, LABEL_MAX, LINK_MAX, NAME_MAX, NOTES_MAX, Platform, Step, SvgProblem,
    TEXT_MAX, TITLE_MAX,
};
use sn_domain::ids::AdminId;
use sn_domain::image::{self, ImageProblem};
use sn_domain::settings::keys;
use sn_remnawave::RemnaError;
use sn_store::{Db, SiteAsset};
use std::collections::BTreeMap;

pub const ICON_PREFIX: &str = "connect.";
pub const INVALID: &str = "connect_apps.invalid";
const STALE_AFTER_SECONDS: i64 = 3600;
const STORED_ICONS_MAX: usize = 400;
const VERSION_LEN: usize = 16;

pub fn icon_asset(tag: &str) -> String {
    format!("{ICON_PREFIX}{tag}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub apps: usize,
    pub name: usize,
    pub label: usize,
    pub title: usize,
    pub text: usize,
    pub link: usize,
    pub buttons: usize,
    pub notes: usize,
    pub icon_bytes: usize,
    pub svg_bytes: usize,
    pub icon_min_side: u32,
}

pub const LIMITS: Limits = Limits {
    apps: APPS_MAX,
    name: NAME_MAX,
    label: LABEL_MAX,
    title: TITLE_MAX,
    text: TEXT_MAX,
    link: LINK_MAX,
    buttons: BUTTONS_MAX,
    notes: NOTES_MAX,
    icon_bytes: ICON_RULES.max_bytes,
    svg_bytes: ICON_SVG_MAX,
    icon_min_side: ICON_RULES.min_side,
};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConnectAppsView {
    pub apps: ConnectApps,
    pub standard: bool,
    pub defaults: ConnectApps,
    pub limits: Limits,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConnectAppsUpdate {
    #[serde(default)]
    pub apps: Option<ConnectApps>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportRequest {
    pub page_config: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UploadedIcon {
    pub tag: String,
    pub kind: IconKind,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportedPlatform {
    pub platform: Platform,
    pub title: &'static str,
    pub apps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImportPreview {
    pub name: String,
    pub apps: ConnectApps,
    pub platforms: Vec<ImportedPlatform>,
    pub total: usize,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicApp {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicConnectApps {
    pub version: String,
    pub apps: BTreeMap<Platform, Vec<PublicApp>>,
}

fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::rejected(INVALID, message)
}

fn icon_problem(problem: ImageProblem) -> ServiceError {
    let message = match problem {
        ImageProblem::Empty => "Файл пустой".to_string(),
        ImageProblem::TooHeavy => format!(
            "Файл больше {} КБ: уменьшите картинку",
            ICON_RULES.max_bytes / 1024
        ),
        ImageProblem::Format => "Подходят PNG, JPEG, WebP и SVG".to_string(),
        ImageProblem::Damaged => "Картинка повреждена: сохраните её заново".to_string(),
        ImageProblem::NotSquare => "Значок должен быть квадратным".to_string(),
        ImageProblem::TooSmall => format!("Значок меньше {} точек по стороне", ICON_RULES.min_side),
        ImageProblem::TooLarge => format!("Значок больше {} точек по стороне", ICON_RULES.max_side),
    };
    ServiceError::rejected("connect_apps.icon", message)
}

fn svg_problem(problem: SvgProblem) -> ServiceError {
    let text = problem.text();
    let mut chars = text.chars();
    let message = chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect::<String>())
        .unwrap_or_default();
    ServiceError::rejected("connect_apps.icon", message)
}

fn summary(apps: &ConnectApps) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = apps
        .0
        .iter()
        .filter(|(_, list)| !list.is_empty())
        .map(|(platform, list)| {
            let names: Vec<String> = list
                .iter()
                .map(|app| {
                    if app.enabled {
                        app.name.clone()
                    } else {
                        format!("{} (выключено)", app.name)
                    }
                })
                .collect();
            (platform.remnawave_key().to_string(), json!(names))
        })
        .collect();
    serde_json::Value::Object(map)
}

async fn put_icon(
    db: &mut impl Db,
    bytes: &[u8],
    kind: IconKind,
    keep: &[String],
    admin: AdminId,
) -> ServiceResult<AppIcon> {
    let digest = hex::encode(Sha256::digest(bytes));
    let tag = digest[..ICON_TAG_LEN].to_string();
    let name = icon_asset(&tag);
    let limit = i64::try_from(STORED_ICONS_MAX).unwrap_or(i64::MAX);
    if !db.site_assets().exists(&name).await?
        && db.site_assets().count_prefix(ICON_PREFIX).await? >= limit
    {
        db.site_assets()
            .prune(ICON_PREFIX, keep, STALE_AFTER_SECONDS)
            .await?;
        if db.site_assets().count_prefix(ICON_PREFIX).await? >= limit {
            return Err(ServiceError::rejected(
                "connect_apps.icons_limit",
                format!(
                    "Загружено уже {STORED_ICONS_MAX} значков — это предел. \
                     Значки, которых нет в списке приложений, удаляются через час"
                ),
            ));
        }
    }
    db.site_assets()
        .put(
            &name,
            &SiteAsset {
                content_type: kind.content_type().to_string(),
                bytes: bytes.to_vec(),
                sha256: digest,
            },
            Some(admin),
        )
        .await?;
    Ok(AppIcon { tag, kind })
}

pub struct ConnectAppsOps<'a> {
    services: &'a Services,
}

impl<'a> ConnectAppsOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        ConnectAppsOps { services }
    }

    async fn saved(&self) -> ServiceResult<()> {
        self.services.reload().await?;
        self.services.notify_settings_changed().await
    }

    pub fn current(&self) -> ConnectApps {
        self.services
            .settings()
            .connect_apps
            .clone()
            .unwrap_or_else(ConnectApps::standard)
    }

    fn referenced(&self) -> Vec<String> {
        self.current()
            .icon_tags()
            .iter()
            .map(|tag| icon_asset(tag))
            .collect()
    }

    pub fn view(&self) -> ConnectAppsView {
        ConnectAppsView {
            apps: self.current().full(),
            standard: self.services.settings().connect_apps.is_none(),
            defaults: ConnectApps::standard(),
            limits: LIMITS,
        }
    }

    pub async fn update(
        &self,
        update: &ConnectAppsUpdate,
        admin: AdminId,
    ) -> ServiceResult<ConnectAppsView> {
        let next = match &update.apps {
            Some(apps) => apps.clone().checked().map_err(invalid)?,
            None => ConnectApps::standard(),
        };
        {
            let mut conn = self.services.store().conn().await?;
            for (label, icon) in next.icons() {
                let asset = conn.site_assets().get(&icon_asset(&icon.tag)).await?;
                let found = asset
                    .filter(|asset| asset.sha256.starts_with(&icon.tag))
                    .and_then(|asset| IconKind::sniff(&asset.bytes))
                    .is_some_and(|kind| kind == icon.kind);
                if !found {
                    return Err(invalid(format!(
                        "{label}: значок не найден, загрузите его заново"
                    )));
                }
            }
        }
        let standard = next == ConnectApps::standard();
        let stored = self.services.settings().connect_apps.is_some();
        if next == self.current() && stored != standard {
            return Ok(self.view());
        }
        let keep: Vec<String> = next.icon_tags().iter().map(|tag| icon_asset(tag)).collect();
        let mut tx = self.services.store().begin().await?;
        if standard {
            tx.settings().delete(keys::CONNECT_APPS).await?;
        } else {
            tx.settings()
                .set_as(keys::CONNECT_APPS, &next, Some(admin))
                .await?;
        }
        let removed = tx
            .site_assets()
            .prune(ICON_PREFIX, &keep, STALE_AFTER_SECONDS)
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.connect_apps",
                "settings",
                None,
                json!({
                    "standard": standard,
                    "apps": summary(&next),
                    "removed_icons": removed,
                }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.view())
    }

    pub async fn upload_icon(
        &self,
        bytes: &[u8],
        svg: bool,
        admin: AdminId,
    ) -> ServiceResult<UploadedIcon> {
        let (bytes, kind) = if svg {
            connect::check_svg(bytes).map_err(svg_problem)?;
            let text = std::str::from_utf8(bytes).map_err(|_| svg_problem(SvgProblem::Format))?;
            let fixed = connect::with_svg_namespace(text).into_bytes();
            connect::check_svg(&fixed).map_err(svg_problem)?;
            (fixed, IconKind::Svg)
        } else {
            let info = image::check(bytes, &ICON_RULES).map_err(icon_problem)?;
            let kind = IconKind::from_image(info.kind)
                .ok_or_else(|| icon_problem(ImageProblem::Format))?;
            (bytes.to_vec(), kind)
        };
        let keep = self.referenced();
        let mut tx = self.services.store().begin().await?;
        let icon = put_icon(&mut tx, &bytes, kind, &keep, admin).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.connect_app_icon",
                "settings",
                None,
                json!({
                    "kind": icon.kind,
                    "size": bytes.len(),
                    "tag": icon.tag,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(UploadedIcon {
            url: icon.path(),
            tag: icon.tag,
            kind: icon.kind,
        })
    }

    pub async fn icon_file(
        &self,
        tag: &str,
        extension: &str,
    ) -> ServiceResult<Option<(IconKind, Vec<u8>)>> {
        if !connect::valid_icon_tag(tag) {
            return Ok(None);
        }
        let mut conn = self.services.store().conn().await?;
        let Some(asset) = conn.site_assets().get(&icon_asset(tag)).await? else {
            return Ok(None);
        };
        if !asset.sha256.starts_with(tag) {
            return Ok(None);
        }
        let kind = IconKind::sniff(&asset.bytes).filter(|kind| kind.extension() == extension);
        Ok(kind.map(|kind| (kind, asset.bytes)))
    }

    pub async fn import(
        &self,
        request: &ImportRequest,
        admin: AdminId,
    ) -> ServiceResult<ImportPreview> {
        let uuid = uuid::Uuid::parse_str(request.page_config.trim())
            .map_err(|_| invalid("нужен идентификатор оформления страницы"))?;
        let remna = self.services.remna()?;
        let config = match remna.subscription_page_config(uuid).await {
            Ok(config) => config,
            Err(RemnaError::NotFound { .. }) => {
                return Err(ServiceError::rejected(
                    "connect_apps.not_found",
                    "Такого оформления в Remnawave больше нет: обновите страницу и выберите другое",
                ));
            }
            Err(err) => return Err(err.into()),
        };
        let content = config
            .config
            .ok_or_else(|| invalid("Оформление пустое: переносить нечего"))?;
        let imported = connect::from_remnawave(&content).map_err(invalid)?;
        let keep = self.referenced();
        let mut grouped: BTreeMap<Platform, Vec<ConnectApp>> = BTreeMap::new();
        let mut icons = 0usize;
        let mut tx = self.services.store().begin().await?;
        for item in imported.apps {
            let mut app = item.app;
            if let Some(svg) = item.svg {
                app.icon =
                    Some(put_icon(&mut tx, svg.as_bytes(), IconKind::Svg, &keep, admin).await?);
                icons += 1;
            }
            grouped.entry(item.platform).or_default().push(app);
        }
        let apps = ConnectApps(grouped).checked().map_err(invalid)?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.connect_apps_import",
                "settings",
                None,
                json!({
                    "page_config": uuid.to_string(),
                    "name": config.name,
                    "apps": apps.count(),
                    "icons": icons,
                }),
            ))
            .await?;
        tx.commit().await?;
        let platforms = Platform::ALL
            .into_iter()
            .filter_map(|platform| {
                let list = apps.on(platform);
                (!list.is_empty()).then(|| ImportedPlatform {
                    platform,
                    title: platform.title(),
                    apps: list.iter().map(|app| app.name.clone()).collect(),
                })
            })
            .collect();
        Ok(ImportPreview {
            name: config.name,
            total: apps.count(),
            apps,
            platforms,
            notes: imported.notes,
        })
    }

    pub fn public(&self) -> PublicConnectApps {
        let apps: BTreeMap<Platform, Vec<PublicApp>> = self
            .current()
            .shown()
            .0
            .into_iter()
            .map(|(platform, list)| {
                let list = list
                    .into_iter()
                    .map(|app| PublicApp {
                        icon: app.icon.as_ref().map(AppIcon::path),
                        id: app.id,
                        name: app.name,
                        steps: app.steps,
                    })
                    .collect();
                (platform, list)
            })
            .collect();
        let bytes = serde_json::to_vec(&apps).unwrap_or_default();
        PublicConnectApps {
            version: hex::encode(Sha256::digest(&bytes))[..VERSION_LEN].to_string(),
            apps,
        }
    }
}
