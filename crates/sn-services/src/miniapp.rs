use crate::Actor;
use crate::bot_layout::miniapp_off;
use crate::error::{ServiceError, ServiceResult};
use crate::landing::ImageRulesView;
use crate::services::Services;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sn_domain::ids::AdminId;
use sn_domain::image::{self, ImageKind, ImageProblem, LANDING_RULES};
use sn_domain::landing::{IMAGE_TAG_LEN, valid_tag};
use sn_domain::miniapp::{self, LIMITS, Limits, MiniAppConfig, STORED_IMAGES_MAX, Section};
use sn_domain::module::ModuleId;
use sn_domain::settings::keys;
use sn_modules::catalog::referral::ReferralSettings;
use sn_modules::catalog::roulette;
use sn_store::{Db, SiteAsset};

pub const ASSET_PREFIX: &str = "miniapp.";
pub const STALE_AFTER_SECONDS: i64 = 3600;
pub const INVALID: &str = "miniapp.invalid";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SectionView {
    pub key: Section,
    pub title: &'static str,
    pub off: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MiniAppView {
    #[serde(flatten)]
    pub config: MiniAppConfig,
    pub limits: Limits,
    pub image: ImageRulesView,
    pub sections: Vec<SectionView>,
    pub english: bool,
    pub off: Option<&'static str>,
    pub friends_percent: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UploadedImage {
    pub tag: String,
    pub kind: ImageKind,
    pub width: u32,
    pub height: u32,
    pub size: u32,
    pub url: String,
}

pub fn asset_name(tag: &str) -> String {
    format!("{ASSET_PREFIX}{tag}")
}

fn image_problem(problem: ImageProblem) -> ServiceError {
    let (code, message) = match problem {
        ImageProblem::Empty => ("miniapp_image.empty", "Файл пустой".to_string()),
        ImageProblem::TooHeavy => (
            "miniapp_image.too_heavy",
            "Файл больше 1 МБ: уменьшите картинку".to_string(),
        ),
        ImageProblem::Format | ImageProblem::NotSquare => (
            "miniapp_image.format",
            "Подходят только PNG, JPEG и WebP".to_string(),
        ),
        ImageProblem::Damaged => (
            "miniapp_image.damaged",
            "Картинка повреждена или сохранена не до конца".to_string(),
        ),
        ImageProblem::TooSmall => (
            "miniapp_image.too_small",
            format!(
                "Картинка слишком маленькая: нужно от {} точек по каждой стороне",
                LANDING_RULES.min_side
            ),
        ),
        ImageProblem::TooLarge => (
            "miniapp_image.too_large",
            format!(
                "Картинка больше {} точек по стороне",
                LANDING_RULES.max_side
            ),
        ),
    };
    ServiceError::rejected(code, message)
}

pub struct MiniAppOps<'a> {
    services: &'a Services,
}

impl<'a> MiniAppOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        MiniAppOps { services }
    }

    async fn saved(&self) -> ServiceResult<()> {
        self.services.reload().await?;
        self.services.notify_settings_changed().await
    }

    pub fn config(&self) -> MiniAppConfig {
        self.services.settings().miniapp.clone()
    }

    fn module_off(&self, id: ModuleId) -> Option<String> {
        (!self.services.modules().is_enabled(id))
            .then(|| format!("Модуль «{}» выключен", id.info().title))
    }

    fn balance_off(&self) -> Option<String> {
        (!self.services.payments().balance_enabled())
            .then(|| "Пополнение баланса выключено в «Оплата»".to_string())
    }

    fn tab_off(&self, section: Section) -> Option<String> {
        match section {
            Section::Friends => self.module_off(ModuleId::Referral),
            Section::Gifts => self.module_off(ModuleId::Gifts),
            Section::Promo => self.module_off(ModuleId::Promo),
            Section::Topup => self.balance_off(),
            _ => None,
        }
    }

    async fn roulette_off(&self) -> Option<String> {
        if let Some(reason) = self.module_off(ModuleId::Roulette) {
            return Some(reason);
        }
        let ready = roulette::ready(&self.services.module_context())
            .await
            .unwrap_or(false);
        (!ready).then(|| "Рулетка ещё не настроена".to_string())
    }

    async fn off(&self, section: Section) -> Option<String> {
        if let Some(reason) = self.tab_off(section) {
            return Some(reason);
        }
        match section {
            Section::Roulette => self.roulette_off().await,
            Section::Addons => self.module_off(ModuleId::Options),
            Section::Trial => (!self.services.settings().trial.is_offered())
                .then(|| "Пробный период выключен в «Тарифы»".to_string()),
            _ => None,
        }
    }

    fn friends_percent(&self) -> u8 {
        let config = self.services.modules().config(ModuleId::Referral);
        ReferralSettings::lenient(&config, self.services.settings().currency).levels[0]
    }

    pub async fn view(&self) -> ServiceResult<MiniAppView> {
        let settings = self.services.settings();
        let mut sections = Vec::with_capacity(Section::ALL.len());
        for key in Section::ALL {
            sections.push(SectionView {
                key,
                title: key.title(),
                off: self.off(key).await,
            });
        }
        Ok(MiniAppView {
            config: settings.miniapp.with_urls(),
            limits: LIMITS,
            image: ImageRulesView::landing(),
            sections,
            english: settings.languages.several(),
            off: miniapp_off(self.services),
            friends_percent: self.friends_percent(),
        })
    }

    pub async fn public(&self) -> Option<MiniAppConfig> {
        let settings = self.services.settings();
        if settings.miniapp.is_standard() {
            return None;
        }
        let mut config = settings.miniapp.published();
        let mut tabs = Vec::with_capacity(config.tabs.len());
        for tab in config.tabs {
            let off = match tab.std {
                Section::Roulette => self.roulette_off().await,
                section => self.tab_off(section),
            };
            if off.is_none() {
                tabs.push(tab);
            }
        }
        config.tabs = tabs;
        Some(config)
    }

    fn referenced(config: &MiniAppConfig) -> Vec<String> {
        config
            .image_tags()
            .iter()
            .map(|tag| asset_name(tag))
            .collect()
    }

    async fn measure(&self, config: &mut MiniAppConfig) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        for (place, image) in config.images_mut() {
            let asset = conn.site_assets().get(&asset_name(&image.tag)).await?;
            let info = asset
                .filter(|asset| asset.sha256.starts_with(&image.tag))
                .and_then(|asset| image::inspect(&asset.bytes))
                .filter(|info| LANDING_RULES.kinds.contains(&info.kind));
            let Some(info) = info else {
                return Err(ServiceError::rejected(
                    INVALID,
                    format!("{place}: не найдена, загрузите её заново"),
                ));
            };
            image.kind = Some(info.kind);
            image.width = info.width;
            image.height = info.height;
        }
        Ok(())
    }

    pub async fn update(&self, raw: &Value, admin: AdminId) -> ServiceResult<MiniAppView> {
        let mut next = MiniAppConfig::parse(raw)
            .map_err(|err| ServiceError::rejected(INVALID, err.to_string()))?;
        self.measure(&mut next).await?;
        let current = self.config();
        if next == current {
            return self.view().await;
        }
        let diff = current.difference(&next);
        let standard = next.is_standard();
        let keep = Self::referenced(&next);
        let mut tx = self.services.store().begin().await?;
        if standard {
            tx.settings().delete(keys::MINIAPP).await?;
        } else {
            tx.settings()
                .set_as(keys::MINIAPP, &next, Some(admin))
                .await?;
        }
        let removed = tx
            .site_assets()
            .prune(ASSET_PREFIX, &keep, STALE_AFTER_SECONDS)
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.miniapp",
                "settings",
                None,
                json!({
                    "screens": diff.screens,
                    "blocks": diff.blocks,
                    "order": diff.order,
                    "hidden": next.hidden(),
                    "standard": standard,
                    "removed_images": removed,
                }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        self.view().await
    }

    pub async fn upload_image(&self, bytes: &[u8], admin: AdminId) -> ServiceResult<UploadedImage> {
        let info = image::check(bytes, &LANDING_RULES).map_err(image_problem)?;
        let digest = hex::encode(Sha256::digest(bytes));
        let tag = digest[..IMAGE_TAG_LEN].to_string();
        let name = asset_name(&tag);
        let size = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        let limit = i64::try_from(STORED_IMAGES_MAX).unwrap_or(i64::MAX);
        let mut tx = self.services.store().begin().await?;
        if !tx.site_assets().exists(&name).await?
            && tx.site_assets().count_prefix(ASSET_PREFIX).await? >= limit
        {
            let keep = Self::referenced(&self.config());
            tx.site_assets()
                .prune(ASSET_PREFIX, &keep, STALE_AFTER_SECONDS)
                .await?;
            if tx.site_assets().count_prefix(ASSET_PREFIX).await? >= limit {
                return Err(ServiceError::rejected(
                    "miniapp_image.limit",
                    format!(
                        "Для мини-приложения уже загружено {STORED_IMAGES_MAX} картинок — это предел. \
                         Картинки, которых нет в баннерах, удаляются через час, после этого загрузка снова заработает"
                    ),
                ));
            }
        }
        tx.site_assets()
            .put(
                &name,
                &SiteAsset {
                    content_type: info.kind.content_type().to_string(),
                    bytes: bytes.to_vec(),
                    sha256: digest.clone(),
                },
                Some(admin),
            )
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.miniapp_image",
                "settings",
                None,
                json!({
                    "action": "uploaded",
                    "kind": info.kind,
                    "width": info.width,
                    "height": info.height,
                    "size": size,
                    "sha256": digest,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(UploadedImage {
            url: miniapp::image_path(&tag, info.kind),
            tag,
            kind: info.kind,
            width: info.width,
            height: info.height,
            size,
        })
    }

    pub async fn image_file(&self, tag: &str) -> ServiceResult<Option<(ImageKind, Vec<u8>)>> {
        if !valid_tag(tag) {
            return Ok(None);
        }
        let mut conn = self.services.store().conn().await?;
        let Some(asset) = conn.site_assets().get(&asset_name(tag)).await? else {
            return Ok(None);
        };
        if !asset.sha256.starts_with(tag) {
            return Ok(None);
        }
        let kind = ImageKind::sniff(&asset.bytes).filter(|kind| LANDING_RULES.kinds.contains(kind));
        Ok(kind.map(|kind| (kind, asset.bytes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvFallbacks, ServiceDeps, SystemClock};
    use sn_domain::access::AdminRole;
    use sn_domain::miniapp::{HomeBlock, HomeKind};
    use sn_modules::ModuleRegistry;
    use sn_payments::ProviderRegistry;
    use sn_store::NewAdmin;
    use sn_store::testing::{TestDb, test_store};
    use std::sync::Arc;

    struct Env {
        db: TestDb,
        services: Services,
        admin: AdminId,
    }

    fn services_for(db: &TestDb) -> Services {
        Services::new(ServiceDeps {
            store: db.store.clone(),
            payments: Arc::new(ProviderRegistry::builtin()),
            modules: Arc::new(ModuleRegistry::catalog_only()),
            secrets: None,
            clock: Arc::new(SystemClock),
            env: EnvFallbacks::default(),
        })
    }

    async fn env() -> Option<Env> {
        let db = test_store().await?;
        let services = services_for(&db);
        services.reload().await.unwrap();
        let admin = {
            let mut conn = db.store.conn().await.unwrap();
            conn.admins()
                .create(&NewAdmin {
                    email: "owner@example.com".into(),
                    password_hash: "hash".into(),
                    role: AdminRole::Owner,
                    must_change_password: false,
                })
                .await
                .unwrap()
                .id
        };
        Some(Env {
            db,
            services,
            admin,
        })
    }

    fn png(width: u32, height: u32, fill: u8) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes.extend_from_slice(&[fill; 64]);
        bytes
    }

    async fn audits(env: &Env, action: &str) -> Vec<sn_store::AuditRecord> {
        let mut conn = env.db.store.conn().await.unwrap();
        conn.audit().recent_by_action(action, 50).await.unwrap()
    }

    async fn stored(env: &Env, tag: &str) -> bool {
        let mut conn = env.db.store.conn().await.unwrap();
        conn.site_assets().exists(&asset_name(tag)).await.unwrap()
    }

    async fn saved_setting(env: &Env) -> Option<Value> {
        let mut conn = env.db.store.conn().await.unwrap();
        conn.settings().get(keys::MINIAPP).await.unwrap()
    }

    async fn age(env: &Env, tags: &[&str]) {
        let names: Vec<String> = tags.iter().map(|tag| asset_name(tag)).collect();
        let mut conn = env.db.store.conn().await.unwrap();
        sqlx::query(
            "UPDATE site_assets SET updated_at = now() - interval '2 hours' WHERE name = ANY($1)",
        )
        .bind(&names)
        .execute(conn.conn())
        .await
        .unwrap();
    }

    fn rejected<T: std::fmt::Debug>(result: ServiceResult<T>) -> (&'static str, String) {
        match result.unwrap_err() {
            ServiceError::Rejected { code, message } => (code, message),
            other => panic!("ожидался отказ, пришло {other:?}"),
        }
    }

    #[tokio::test]
    async fn настройки_сохраняются_читаются_после_перезапуска_и_сбрасываются() {
        let Some(env) = env().await else { return };
        let ops = env.services.miniapp();
        let first = ops.view().await.unwrap();
        assert_eq!(first.config, MiniAppConfig::standard());
        assert_eq!(first.limits.tabs_max, 5);
        assert_eq!(first.image.max_bytes, 1024 * 1024);
        assert_eq!(first.friends_percent, 30);
        assert!(!first.english);
        assert_eq!(ops.public().await, None);

        let saved = ops
            .update(
                &json!({
                    "header": { "balance": false },
                    "home": [
                        { "kind": "sections", "items": [{ "std": "support" }, { "std": "guide", "on": false }] },
                        { "kind": "greeting", "on": true, "title": { "ru": " Привет,  {name}! " } },
                        { "kind": "actions", "on": false }
                    ],
                    "tabs": [
                        { "std": "home" }, { "std": "plans", "label": { "ru": "Купить" } },
                        { "std": "support" }, { "std": "profile" }
                    ],
                    "profile": [{ "kind": "friends", "place": "screen" }]
                }),
                env.admin,
            )
            .await
            .unwrap();
        assert!(!saved.config.header.balance);
        assert_eq!(
            saved
                .config
                .home
                .iter()
                .map(HomeBlock::kind)
                .collect::<Vec<_>>(),
            [
                HomeKind::Banners,
                HomeKind::Sections,
                HomeKind::Greeting,
                HomeKind::Subscription,
                HomeKind::Actions
            ]
        );
        let shown = ops.public().await.unwrap();
        assert_eq!(shown.tabs, saved.config.tabs);
        assert_eq!(shown.profile, saved.config.profile);
        assert_eq!(shown.header, saved.config.header);
        let HomeBlock::Actions(actions) = &shown.home[4] else {
            unreachable!()
        };
        assert!(actions.items.is_empty());

        let fresh = services_for(&env.db);
        fresh.reload().await.unwrap();
        assert_eq!(fresh.miniapp().config(), env.services.miniapp().config());
        let raw = saved_setting(&env).await.unwrap();
        assert_eq!(raw["header"], json!({ "balance": false }));
        assert_eq!(raw["home"][2]["title"], json!({ "ru": "Привет, {name}!" }));

        let records = audits(&env, "settings.miniapp").await;
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].details["screens"],
            json!(["home", "tabs", "profile"])
        );
        assert_eq!(
            records[0].details["blocks"],
            json!(["header", "greeting", "actions", "sections", "friends"])
        );
        assert_eq!(records[0].details["order"], true);
        assert_eq!(records[0].details["hidden"], json!(["actions"]));
        assert_eq!(records[0].details["standard"], false);
        let text = records[0].details.to_string();
        assert!(!text.contains("Привет"), "{text}");
        assert!(text.len() < 400, "{text}");

        let again = ops
            .update(&serde_json::to_value(&saved.config).unwrap(), env.admin)
            .await
            .unwrap();
        assert_eq!(again.config, saved.config);
        assert_eq!(audits(&env, "settings.miniapp").await.len(), 1);

        let reset = ops.update(&json!({}), env.admin).await.unwrap();
        assert_eq!(reset.config, MiniAppConfig::standard());
        assert_eq!(ops.public().await, None);
        assert!(saved_setting(&env).await.is_none());
        let records = audits(&env, "settings.miniapp").await;
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].details["standard"], true);

        let (code, message) = rejected(
            ops.update(
                &json!({ "tabs": [{ "std": "home" }, { "std": "profile" }] }),
                env.admin,
            )
            .await,
        );
        assert_eq!(code, INVALID);
        assert_eq!(
            message,
            "Меню внизу: вкладку «Тарифы» убрать нельзя: без неё клиент не оплатит подписку"
        );
        assert_eq!(audits(&env, "settings.miniapp").await.len(), 2);
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn испорченная_настройка_не_ломает_мини_приложение() {
        let Some(env) = env().await else { return };
        {
            let mut conn = env.db.store.conn().await.unwrap();
            conn.settings()
                .set_as(
                    keys::MINIAPP,
                    &json!({ "home": [{ "kind": "unknown" }] }),
                    Some(env.admin),
                )
                .await
                .unwrap();
        }
        env.services.reload().await.unwrap();
        assert!(env.services.miniapp().config().is_standard());
        assert_eq!(env.services.miniapp().public().await, None);
        {
            let mut conn = env.db.store.conn().await.unwrap();
            conn.settings()
                .set_as(
                    keys::MINIAPP,
                    &json!({ "tabs": [{ "std": "gifts" }], "header": { "balance": false } }),
                    Some(env.admin),
                )
                .await
                .unwrap();
        }
        env.services.reload().await.unwrap();
        let config = env.services.miniapp().config();
        assert!(!config.header.balance);
        assert_eq!(config.tabs, MiniAppConfig::standard().tabs);
        assert_eq!(config.home.len(), HomeKind::ALL.len());
        env.db.cleanup().await;
    }

    async fn saved(env: &Env) -> Option<Value> {
        let mut conn = env.db.store.conn().await.unwrap();
        conn.settings()
            .get_as::<Value>(keys::MINIAPP)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn вид_с_незнакомыми_частями_читается_а_лишнее_уходит_при_правке() {
        let Some(env) = env().await else { return };
        let raw = json!({
            "home": [
                { "kind": "stories", "items": [{ "title": { "ru": "История" } }] },
                { "kind": "greeting", "on": true, "title": { "ru": "Привет" } },
                { "kind": "actions", "items": [{ "std": "club" }, { "std": "support" }] }
            ],
            "tabs": [
                { "std": "home" }, { "std": "club" }, { "std": "plans" }, { "std": "profile" }
            ],
            "profile": [{ "kind": "awards" }, { "kind": "account" }]
        });
        {
            let mut conn = env.db.store.conn().await.unwrap();
            conn.settings()
                .set_as(keys::MINIAPP, &raw, Some(env.admin))
                .await
                .unwrap();
        }
        env.services.reload().await.unwrap();
        let ops = env.services.miniapp();
        assert!(!ops.config().is_standard());
        let public = ops.public().await.unwrap();
        let tabs: Vec<Section> = public.tabs.iter().map(|tab| tab.std).collect();
        assert_eq!(tabs, [Section::Home, Section::Plans, Section::Profile]);
        let text = serde_json::to_string(&public).unwrap();
        assert!(text.contains("Привет"), "{text}");
        for gone in ["stories", "club", "awards"] {
            assert!(!text.contains(gone), "{text}");
        }

        let view = serde_json::to_value(ops.view().await.unwrap()).unwrap();
        ops.update(&view, env.admin).await.unwrap();
        assert_eq!(saved(&env).await, Some(raw));
        assert!(audits(&env, "settings.miniapp").await.is_empty());

        let mut next = view;
        next["header"] = json!({ "balance": false });
        ops.update(&next, env.admin).await.unwrap();
        let text = saved(&env).await.unwrap().to_string();
        assert!(text.contains("Привет"), "{text}");
        for gone in ["stories", "club", "awards"] {
            assert!(!text.contains(gone), "{text}");
        }
        assert_eq!(audits(&env, "settings.miniapp").await.len(), 1);
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn недоступные_разделы_названы_а_их_вкладки_клиенту_не_отдаются() {
        let Some(env) = env().await else { return };
        let ops = env.services.miniapp();
        let view = ops.view().await.unwrap();
        let off = |key: Section| {
            view.sections
                .iter()
                .find(|section| section.key == key)
                .unwrap()
                .off
                .clone()
        };
        assert_eq!(view.sections.len(), Section::ALL.len());
        for key in [
            Section::Home,
            Section::Plans,
            Section::Connect,
            Section::Profile,
            Section::Payments,
            Section::Guide,
            Section::Support,
            Section::Renew,
            Section::Devices,
        ] {
            assert_eq!(off(key), None, "{key:?}");
        }
        let module = |id: ModuleId| {
            (!env.services.modules().is_enabled(id))
                .then(|| format!("Модуль «{}» выключен", id.info().title))
        };
        assert_eq!(off(Section::Friends), module(ModuleId::Referral));
        assert_eq!(off(Section::Gifts), module(ModuleId::Gifts));
        assert_eq!(off(Section::Promo), module(ModuleId::Promo));
        assert_eq!(off(Section::Addons), module(ModuleId::Options));
        assert!(off(Section::Roulette).is_some());
        ops.update(
            &json!({ "tabs": [
                { "std": "home" }, { "std": "plans" }, { "std": "roulette" }, { "std": "profile" }
            ] }),
            env.admin,
        )
        .await
        .unwrap();
        let names: Vec<&str> = ops
            .public()
            .await
            .unwrap()
            .tabs
            .iter()
            .map(|tab| tab.std.as_str())
            .collect();
        assert_eq!(names, ["home", "plans", "profile"]);
        assert_eq!(ops.config().tabs.len(), 4);
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn картинка_баннера_получает_размеры_из_файла_а_чужая_не_принимается() {
        let Some(env) = env().await else { return };
        let ops = env.services.miniapp();
        let first = ops
            .upload_image(&png(1432, 536, 1), env.admin)
            .await
            .unwrap();
        assert_eq!(first.tag.len(), 16);
        assert_eq!((first.width, first.height), (1432, 536));
        assert_eq!(first.url, format!("/api/public/miniapp/{}.png", first.tag));
        let (kind, bytes) = ops.image_file(&first.tag).await.unwrap().unwrap();
        assert_eq!(kind, ImageKind::Png);
        assert_eq!(bytes, png(1432, 536, 1));
        assert!(ops.image_file("ffffffffffffffff").await.unwrap().is_none());
        assert!(ops.image_file("../../etc/passwd").await.unwrap().is_none());
        assert!(
            env.services
                .landing()
                .image_file(&first.tag)
                .await
                .unwrap()
                .is_none()
        );

        let saved = ops
            .update(
                &json!({ "home": [{ "kind": "banners", "items": [
                    { "image": { "tag": first.tag.to_uppercase(), "width": 1, "kind": "webp" }, "to": "plans" }
                ] }] }),
                env.admin,
            )
            .await
            .unwrap();
        let image = saved.config.banners()[0].image.clone().unwrap();
        assert_eq!(image.kind, Some(ImageKind::Png));
        assert_eq!((image.width, image.height), (1432, 536));
        assert_eq!(image.url.as_deref(), Some(first.url.as_str()));
        let raw = saved_setting(&env).await.unwrap();
        assert!(raw["home"][0]["items"][0]["image"].get("url").is_none());

        let (code, message) = rejected(
            ops.update(
                &json!({ "home": [{ "kind": "banners", "items": [
                    { "title": { "ru": "Скидка" }, "image_en": { "tag": "ffffffffffffffff" } }
                ] }] }),
                env.admin,
            )
            .await,
        );
        assert_eq!(code, INVALID);
        assert_eq!(
            message,
            "Главная, баннер 1, картинка на английском: не найдена, загрузите её заново"
        );
        for (bytes, code) in [
            (Vec::new(), "miniapp_image.empty"),
            (png(50, 50, 1), "miniapp_image.too_small"),
            (png(5000, 400, 1), "miniapp_image.too_large"),
            (
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(),
                "miniapp_image.format",
            ),
            (png(800, 600, 1)[..14].to_vec(), "miniapp_image.damaged"),
        ] {
            assert_eq!(rejected(ops.upload_image(&bytes, env.admin).await).0, code);
        }
        let mut heavy = png(800, 600, 3);
        heavy.resize(1024 * 1024 + 1, 0);
        assert_eq!(
            rejected(ops.upload_image(&heavy, env.admin).await).0,
            "miniapp_image.too_heavy"
        );
        assert_eq!(audits(&env, "settings.miniapp_image").await.len(), 1);
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn ненужные_картинки_удаляются_а_картинки_лендинга_остаются() {
        let Some(env) = env().await else { return };
        let ops = env.services.miniapp();
        let landing = env
            .services
            .landing()
            .upload_image(&png(800, 600, 9), env.admin)
            .await
            .unwrap()
            .tag;
        let kept = ops
            .upload_image(&png(800, 600, 1), env.admin)
            .await
            .unwrap()
            .tag;
        let unused = ops
            .upload_image(&png(800, 600, 2), env.admin)
            .await
            .unwrap()
            .tag;
        let fresh = ops
            .upload_image(&png(800, 600, 3), env.admin)
            .await
            .unwrap()
            .tag;
        age(&env, &[&kept, &unused]).await;
        ops.update(
            &json!({ "home": [{ "kind": "banners", "items": [{ "image": { "tag": kept } }] }] }),
            env.admin,
        )
        .await
        .unwrap();
        assert!(stored(&env, &kept).await);
        assert!(!stored(&env, &unused).await);
        assert!(stored(&env, &fresh).await);
        assert_eq!(
            audits(&env, "settings.miniapp").await[0].details["removed_images"],
            1
        );
        ops.update(&json!({}), env.admin).await.unwrap();
        assert!(!stored(&env, &kept).await);
        assert!(
            env.services
                .landing()
                .image_file(&landing)
                .await
                .unwrap()
                .is_some()
        );

        for fill in 10..STORED_IMAGES_MAX + 10 - 1 {
            ops.upload_image(&png(400, 300, u8::try_from(fill).unwrap()), env.admin)
                .await
                .unwrap();
        }
        let (code, message) = rejected(ops.upload_image(&png(400, 300, 200), env.admin).await);
        assert_eq!(code, "miniapp_image.limit");
        assert!(message.contains("40"), "{message}");
        assert!(ops.upload_image(&png(800, 600, 3), env.admin).await.is_ok());
        env.db.cleanup().await;
    }
}
