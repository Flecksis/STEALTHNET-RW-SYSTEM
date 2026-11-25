use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::own_css::{CssCheck, CssPlaces, CssReport, OwnCss, StyleFile, short_hash};
use crate::services::Services;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sn_domain::ids::AdminId;
use sn_domain::image::{self, ImageKind, ImageProblem, LANDING_RULES};
use sn_domain::landing::{
    self, Block, BlockKind, IMAGE_TAG_LEN, LIMITS, LandingConfig, Limits, STORED_IMAGES_MAX,
};
use sn_domain::landing_css::{CssPlace, LandingCss};
use sn_domain::settings::keys;
use sn_store::{Db, SiteAsset};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const ASSET_PREFIX: &str = "landing.";
pub const STALE_AFTER_SECONDS: i64 = 3600;
pub const INVALID: &str = "landing.invalid";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImageRulesView {
    pub max_bytes: usize,
    pub min_side: u32,
    pub max_side: u32,
    pub types: Vec<&'static str>,
}

impl ImageRulesView {
    pub fn landing() -> Self {
        ImageRulesView {
            max_bytes: LANDING_RULES.max_bytes,
            min_side: LANDING_RULES.min_side,
            max_side: LANDING_RULES.max_side,
            types: LANDING_RULES
                .kinds
                .iter()
                .map(|kind| kind.content_type())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LandingView {
    pub blocks: Vec<Block>,
    pub css: LandingCss,
    pub styles: BTreeMap<CssPlace, String>,
    pub limits: Limits,
    pub image: ImageRulesView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicLanding {
    pub version: String,
    pub blocks: Vec<Block>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
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

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LandingUpdate {
    #[serde(default)]
    pub blocks: Option<Vec<Value>>,
    #[serde(default)]
    pub css: Option<CssPlaces>,
}

pub fn asset_name(tag: &str) -> String {
    format!("{ASSET_PREFIX}{tag}")
}

pub fn version_of(blocks: &[Block], css: Option<&str>) -> String {
    let mut bytes = serde_json::to_vec(blocks).unwrap_or_default();
    bytes.extend_from_slice(css.unwrap_or_default().as_bytes());
    short_hash(&bytes)
}

fn image_problem(problem: ImageProblem) -> ServiceError {
    let (code, message) = match problem {
        ImageProblem::Empty => ("landing_image.empty", "Файл пустой".to_string()),
        ImageProblem::TooHeavy => (
            "landing_image.too_heavy",
            "Файл больше 1 МБ: уменьшите картинку".to_string(),
        ),
        ImageProblem::Format | ImageProblem::NotSquare => (
            "landing_image.format",
            "Подходят только PNG, JPEG и WebP".to_string(),
        ),
        ImageProblem::Damaged => (
            "landing_image.damaged",
            "Картинка повреждена или сохранена не до конца".to_string(),
        ),
        ImageProblem::TooSmall => (
            "landing_image.too_small",
            format!(
                "Картинка слишком маленькая: нужно от {} точек по каждой стороне",
                LANDING_RULES.min_side
            ),
        ),
        ImageProblem::TooLarge => (
            "landing_image.too_large",
            format!(
                "Картинка больше {} точек по стороне",
                LANDING_RULES.max_side
            ),
        ),
    };
    ServiceError::rejected(code, message)
}

fn block_of(config: &LandingConfig, kind: BlockKind) -> Option<&Block> {
    config.blocks.iter().find(|block| block.kind() == kind)
}

pub struct LandingOps<'a> {
    services: &'a Services,
}

impl<'a> LandingOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        LandingOps { services }
    }

    async fn saved(&self) -> ServiceResult<()> {
        self.services.reload().await?;
        self.services.notify_settings_changed().await
    }

    pub fn config(&self) -> LandingConfig {
        self.services.settings().landing.clone()
    }

    pub fn view(&self) -> LandingView {
        let config = self.services.settings().landing.with_urls();
        LandingView {
            blocks: config.blocks,
            styles: config.css.styles(),
            css: config.css,
            limits: LIMITS,
            image: ImageRulesView::landing(),
        }
    }

    pub fn public(&self) -> PublicLanding {
        let blocks = self.services.settings().landing.with_urls().blocks;
        let css = self.style().map(|style| style.path());
        PublicLanding {
            version: version_of(&blocks, css.as_deref()),
            blocks,
            css,
        }
    }

    pub fn style(&self) -> Option<Arc<StyleFile>> {
        self.services.landing_style()
    }

    fn own_css(&self) -> OwnCss<'a> {
        OwnCss::new(self.services, ASSET_PREFIX, INVALID)
    }

    fn referenced(config: &LandingConfig) -> Vec<String> {
        config
            .image_tags()
            .into_iter()
            .chain(config.css.images().into_iter().map(|(_, _, tag)| tag))
            .map(|tag| asset_name(&tag))
            .collect()
    }

    pub async fn check_css(
        &self,
        check: &CssCheck,
    ) -> ServiceResult<BTreeMap<CssPlace, CssReport>> {
        self.own_css().check(check).await
    }

    pub async fn update(
        &self,
        update: &LandingUpdate,
        admin: AdminId,
    ) -> ServiceResult<LandingView> {
        let raw = update.blocks.as_deref().unwrap_or_default();
        let mut next = LandingConfig::parse(raw)
            .map_err(|err| ServiceError::rejected(INVALID, err.to_string()))?;
        next.css = match &update.css {
            Some(places) => self.own_css().parse(places).await?,
            None => self.config().css,
        };
        {
            let mut conn = self.services.store().conn().await?;
            for (place, image) in next.images_mut() {
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
        }
        let current = self.config();
        if next == current {
            return Ok(self.view());
        }
        let changed: Vec<&'static str> = BlockKind::ALL
            .into_iter()
            .filter(|kind| block_of(&current, *kind) != block_of(&next, *kind))
            .map(BlockKind::as_str)
            .collect();
        let hidden: Vec<&'static str> = next
            .blocks
            .iter()
            .filter(|block| !block.on())
            .map(|block| block.kind().as_str())
            .collect();
        let order = !current
            .blocks
            .iter()
            .map(Block::kind)
            .eq(next.blocks.iter().map(Block::kind));
        let css = current.css.changed(&next.css);
        let css_size = next.css.size();
        let standard = next.is_standard();
        let keep = Self::referenced(&next);
        let mut details = json!({
            "changed": changed,
            "order": order,
            "hidden": hidden,
            "standard": standard,
        });
        if !css.is_empty() {
            details["css"] = json!(css);
            details["css_size"] = json!(css_size);
        }
        let mut tx = self.services.store().begin().await?;
        if standard {
            tx.settings().delete(keys::LANDING).await?;
        } else {
            tx.settings()
                .set_as(keys::LANDING, &next, Some(admin))
                .await?;
        }
        let removed = tx
            .site_assets()
            .prune(ASSET_PREFIX, &keep, STALE_AFTER_SECONDS)
            .await?;
        details["removed_images"] = json!(removed);
        tx.audit()
            .record(&Actor::Admin(admin).audit("settings.landing", "settings", None, details))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.view())
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
                    "landing_image.limit",
                    format!(
                        "Для лендинга уже загружено {STORED_IMAGES_MAX} картинок — это предел. \
                         Картинки, которых нет на странице, удаляются через час, после этого загрузка снова заработает"
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
                "settings.landing_image",
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
            url: landing::image_path(&tag, info.kind),
            tag,
            kind: info.kind,
            width: info.width,
            height: info.height,
            size,
        })
    }

    pub async fn image_file(&self, tag: &str) -> ServiceResult<Option<(ImageKind, Vec<u8>)>> {
        if !landing::valid_tag(tag) {
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
    use sn_domain::landing::Art;
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

    fn update(value: Value) -> LandingUpdate {
        serde_json::from_value(value).unwrap()
    }

    fn kinds(blocks: &[Block]) -> Vec<&'static str> {
        blocks.iter().map(|block| block.kind().as_str()).collect()
    }

    async fn audits(env: &Env, action: &str) -> Vec<sn_store::AuditRecord> {
        let mut conn = env.db.store.conn().await.unwrap();
        conn.audit().recent_by_action(action, 50).await.unwrap()
    }

    async fn stored(env: &Env, tag: &str) -> bool {
        let mut conn = env.db.store.conn().await.unwrap();
        conn.site_assets().exists(&asset_name(tag)).await.unwrap()
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
    async fn лендинг_сохраняется_и_читается_после_перезагрузки() {
        let Some(env) = env().await else { return };
        let ops = env.services.landing();
        assert_eq!(ops.view().blocks, LandingConfig::standard().blocks);
        assert_eq!(ops.view().limits.title, 120);
        assert_eq!(ops.view().image.max_bytes, 1024 * 1024);
        let before = ops.public().version;
        let saved = ops
            .update(
                &update(json!({ "blocks": [
                    { "kind": "hero", "title": "  Свой   заголовок " },
                    { "kind": "features" },
                    { "kind": "faq", "title": "Частые вопросы", "items": [
                        { "std": "devices" },
                        { "title": "Есть пробный период?", "text": "Да, три дня" }
                    ] },
                    { "kind": "steps", "on": false },
                    { "kind": "plans" },
                    { "kind": "servers", "apps": ["Happ", "Hiddify"] },
                    { "kind": "cta" },
                    { "kind": "footer", "links": [{ "label": "Канал", "to": "t.me/stealthnet" }] }
                ] })),
                env.admin,
            )
            .await
            .unwrap();
        assert_eq!(
            kinds(&saved.blocks),
            [
                "hero", "features", "faq", "steps", "plans", "servers", "cta", "footer"
            ]
        );
        let Block::Hero(hero) = &saved.blocks[0] else {
            unreachable!()
        };
        assert_eq!(hero.title.as_deref(), Some("Свой заголовок"));
        let Block::Footer(footer) = &saved.blocks[7] else {
            unreachable!()
        };
        assert_eq!(
            footer.links.as_ref().unwrap()[0].to,
            "https://t.me/stealthnet"
        );
        assert!(!saved.blocks[3].on());
        assert_ne!(ops.public().version, before);
        assert_eq!(ops.public().blocks, saved.blocks);

        let fresh = services_for(&env.db);
        fresh.reload().await.unwrap();
        assert_eq!(fresh.landing().view(), saved);
        assert_eq!(fresh.landing().public(), ops.public());
        {
            let mut conn = env.db.store.conn().await.unwrap();
            let raw = conn.settings().get(keys::LANDING).await.unwrap().unwrap();
            assert_eq!(
                raw["blocks"][0],
                json!({ "kind": "hero", "title": "Свой заголовок" })
            );
        }
        let records = audits(&env, "settings.landing").await;
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].details["changed"],
            json!(["hero", "steps", "servers", "faq", "footer"])
        );
        assert_eq!(records[0].details["order"], true);
        assert_eq!(records[0].details["hidden"], json!(["steps"]));

        let again = ops
            .update(
                &update(json!({ "blocks": serde_json::to_value(&saved.blocks).unwrap() })),
                env.admin,
            )
            .await
            .unwrap();
        assert_eq!(again, saved);
        assert_eq!(audits(&env, "settings.landing").await.len(), 1);

        let reset = ops
            .update(&update(json!({ "blocks": [] })), env.admin)
            .await
            .unwrap();
        assert_eq!(reset.blocks, LandingConfig::standard().blocks);
        assert_eq!(ops.public().version, before);
        {
            let mut conn = env.db.store.conn().await.unwrap();
            assert!(conn.settings().get(keys::LANDING).await.unwrap().is_none());
        }
        let records = audits(&env, "settings.landing").await;
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].details["standard"], true);

        let (code, message) = rejected(
            ops.update(
                &update(json!({ "blocks": [{ "kind": "plans", "title": "" }] })),
                env.admin,
            )
            .await,
        );
        assert_eq!(code, INVALID);
        assert_eq!(message, "Тарифы, заголовок: нужно заполнить");
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn картинка_в_блоке_получает_размеры_из_файла() {
        let Some(env) = env().await else { return };
        let ops = env.services.landing();
        let first = ops
            .upload_image(&png(800, 600, 1), env.admin)
            .await
            .unwrap();
        assert_eq!(first.tag.len(), 16);
        assert_eq!(first.kind, ImageKind::Png);
        assert_eq!((first.width, first.height), (800, 600));
        assert_eq!(first.url, format!("/api/public/landing/{}.png", first.tag));
        let again = ops
            .upload_image(&png(800, 600, 1), env.admin)
            .await
            .unwrap();
        assert_eq!(again.tag, first.tag);
        let (kind, bytes) = ops.image_file(&first.tag).await.unwrap().unwrap();
        assert_eq!(kind, ImageKind::Png);
        assert_eq!(bytes, png(800, 600, 1));
        assert!(ops.image_file("ffffffffffffffff").await.unwrap().is_none());
        assert!(ops.image_file("../../etc/passwd").await.unwrap().is_none());

        let saved = ops
            .update(
                &update(json!({ "blocks": [
                    { "kind": "hero", "art": "image", "image": { "tag": first.tag, "width": 1, "kind": "webp" } },
                    { "kind": "features", "items": [
                        { "std": "cabinet" },
                        { "title": "Бот", "image": { "tag": first.tag.to_uppercase() } }
                    ] }
                ] })),
                env.admin,
            )
            .await
            .unwrap();
        let Block::Hero(hero) = &saved.blocks[0] else {
            unreachable!()
        };
        assert_eq!(hero.art, Some(Art::Image));
        let image = hero.image.as_ref().unwrap();
        assert_eq!(image.kind, Some(ImageKind::Png));
        assert_eq!((image.width, image.height), (800, 600));
        assert_eq!(image.url.as_deref(), Some(first.url.as_str()));
        let public = serde_json::to_value(ops.public()).unwrap();
        assert_eq!(public["blocks"][1]["items"][1]["image"]["width"], 800);
        assert_eq!(public["blocks"][1]["items"][1]["image"]["url"], first.url);
        assert_eq!(public.as_object().unwrap().len(), 2);
        {
            let mut conn = env.db.store.conn().await.unwrap();
            let raw = conn.settings().get(keys::LANDING).await.unwrap().unwrap();
            assert!(raw["blocks"][0]["image"].get("url").is_none());
            assert_eq!(raw["blocks"][0]["image"]["height"], 600);
        }

        let (code, message) = rejected(
            ops.update(
                &update(json!({ "blocks": [
                    { "kind": "hero", "art": "image", "image": { "tag": "ffffffffffffffff" } }
                ] })),
                env.admin,
            )
            .await,
        );
        assert_eq!(code, INVALID);
        assert_eq!(
            message,
            "Первый экран, картинка: не найдена, загрузите её заново"
        );
        assert_eq!(audits(&env, "settings.landing_image").await.len(), 2);

        for (bytes, code) in [
            (Vec::new(), "landing_image.empty"),
            (png(50, 50, 1), "landing_image.too_small"),
            (png(5000, 400, 1), "landing_image.too_large"),
            (
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(),
                "landing_image.format",
            ),
            (png(800, 600, 1)[..14].to_vec(), "landing_image.damaged"),
        ] {
            assert_eq!(rejected(ops.upload_image(&bytes, env.admin).await).0, code);
        }
        let mut heavy = png(800, 600, 3);
        heavy.resize(1024 * 1024 + 1, 0);
        assert_eq!(
            rejected(ops.upload_image(&heavy, env.admin).await).0,
            "landing_image.too_heavy"
        );
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn старые_неиспользуемые_картинки_удаляются_при_сохранении() {
        let Some(env) = env().await else { return };
        let ops = env.services.landing();
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
            &update(json!({ "blocks": [
                { "kind": "hero", "art": "image", "image": { "tag": kept } }
            ] })),
            env.admin,
        )
        .await
        .unwrap();
        assert!(stored(&env, &kept).await);
        assert!(!stored(&env, &unused).await);
        assert!(stored(&env, &fresh).await);
        let records = audits(&env, "settings.landing").await;
        assert_eq!(records[0].details["removed_images"], 1);

        ops.update(&update(json!({ "blocks": [] })), env.admin)
            .await
            .unwrap();
        assert!(!stored(&env, &kept).await);
        assert!(stored(&env, &fresh).await);
        age(&env, &[&fresh]).await;
        ops.update(
            &update(json!({ "blocks": [{ "kind": "plans", "title": "Цены" }] })),
            env.admin,
        )
        .await
        .unwrap();
        assert!(!stored(&env, &fresh).await);
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn свой_css_сохраняется_собирается_в_файл_и_уходит_со_сбросом() {
        let Some(env) = env().await else { return };
        let ops = env.services.landing();
        let plain = ops.public();
        assert!(ops.style().is_none());
        assert_eq!(plain.css, None);
        assert!(ops.view().css.is_empty());

        let saved = ops
            .update(
                &update(json!({
                    "blocks": [{ "kind": "plans", "title": "Цены" }],
                    "css": {
                        "page": ".ld { --accent: teal }  \r\n\r\n",
                        "plans": ".ld-card { gap: 0 }",
                        "faq": "   "
                    }
                })),
                env.admin,
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&saved.css).unwrap(),
            json!({ "page": ".ld { --accent: teal }", "plans": ".ld-card { gap: 0 }" })
        );
        assert_eq!(saved.styles[&CssPlace::Page], ".ld { --accent: teal }\n");
        assert!(saved.styles[&CssPlace::Plans].starts_with("[data-block=\"plans\"] .ld-card"));
        let style = ops.style().unwrap();
        assert_eq!(
            style.text,
            format!(
                "{}{}",
                saved.styles[&CssPlace::Page],
                saved.styles[&CssPlace::Plans]
            )
        );
        assert_eq!(style.version.len(), 16);
        let public = ops.public();
        assert_eq!(
            public.css.as_deref(),
            Some(format!("/api/public/landing/{}.css", style.version).as_str())
        );
        let records = audits(&env, "settings.landing").await;
        assert_eq!(records[0].details["css"], json!(["page", "plans"]));
        assert_eq!(records[0].details["css_size"], 41);
        assert!(!records[0].details.to_string().contains("teal"));

        let fresh = services_for(&env.db);
        fresh.reload().await.unwrap();
        assert_eq!(fresh.landing().view(), saved);
        assert_eq!(fresh.landing().style(), ops.style());

        let kept = ops
            .update(
                &update(json!({ "blocks": [{ "kind": "plans", "title": "Тарифы и цены" }] })),
                env.admin,
            )
            .await
            .unwrap();
        assert_eq!(kept.css, saved.css);
        assert_eq!(ops.style(), Some(style.clone()));
        assert_ne!(ops.public().version, public.version);
        let records = audits(&env, "settings.landing").await;
        assert!(records[0].details.get("css").is_none());
        assert!(records[0].details.get("css_size").is_none());

        let changed = ops
            .update(
                &update(json!({
                    "blocks": [{ "kind": "plans", "title": "Тарифы и цены" }],
                    "css": { "page": ".ld { --accent: navy }" }
                })),
                env.admin,
            )
            .await
            .unwrap();
        assert_eq!(changed.css.get(CssPlace::Plans), "");
        let newer = ops.style().unwrap();
        assert_ne!(newer.version, style.version);
        assert_ne!(ops.public().css, public.css);

        let (code, message) = rejected(
            ops.update(
                &update(json!({ "blocks": [], "css": { "hero": "h1 {\n  color: red;" } })),
                env.admin,
            )
            .await,
        );
        assert_eq!(code, INVALID);
        assert_eq!(
            message,
            "Первый экран, CSS блока, строка 1: не закрыта скобка {"
        );
        assert_eq!(ops.style(), Some(newer));

        let reset = ops
            .update(&update(json!({ "blocks": [], "css": {} })), env.admin)
            .await
            .unwrap();
        assert!(reset.css.is_empty());
        assert!(ops.style().is_none());
        assert_eq!(ops.public(), plain);
        let mut conn = env.db.store.conn().await.unwrap();
        assert!(conn.settings().get(keys::LANDING).await.unwrap().is_none());
        drop(conn);
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn испорченный_в_базе_css_на_сайт_не_попадает() {
        let Some(env) = env().await else { return };
        {
            let mut tx = env.db.store.begin().await.unwrap();
            tx.settings()
                .set_as(
                    keys::LANDING,
                    &json!({
                        "blocks": [{ "kind": "plans", "title": "Цены" }],
                        "css": {
                            "page": "@import 'https://evil.example/x.css';",
                            "hero": "h1 { color: red }",
                            "faq": "p { background: url(https://evil.example/x.png) }",
                            "video": "x",
                            "cta": null
                        }
                    }),
                    None,
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
        env.services.reload().await.unwrap();
        let ops = env.services.landing();
        assert_eq!(
            ops.style().unwrap().text,
            "[data-block=\"hero\"] h1 { color: red }\n"
        );
        let view = ops.view();
        assert_eq!(view.styles.len(), 1);
        assert_eq!(
            view.css.get(CssPlace::Page),
            "@import 'https://evil.example/x.css';"
        );
        let Some(Block::Plans(plans)) = view.blocks.iter().find(|b| b.kind() == BlockKind::Plans)
        else {
            unreachable!()
        };
        assert_eq!(plans.title.as_deref(), Some("Цены"));
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn картинка_из_css_должна_существовать_и_не_удаляется_чисткой() {
        let Some(env) = env().await else { return };
        let ops = env.services.landing();
        let kept = ops
            .upload_image(&png(800, 600, 1), env.admin)
            .await
            .unwrap();
        let unused = ops
            .upload_image(&png(800, 600, 2), env.admin)
            .await
            .unwrap();
        let css = format!("h1 {{}}\nbackground: url(\"{}\");", kept.url);
        age(&env, &[&kept.tag, &unused.tag]).await;
        ops.update(
            &update(json!({ "blocks": [], "css": { "hero": css } })),
            env.admin,
        )
        .await
        .unwrap();
        assert!(stored(&env, &kept.tag).await);
        assert!(!stored(&env, &unused.tag).await);

        let lost = "\n.ld { background: url(/api/public/landing/ffffffffffffffff.png) }";
        let (code, message) = rejected(
            ops.update(
                &update(json!({ "blocks": [], "css": { "page": lost } })),
                env.admin,
            )
            .await,
        );
        assert_eq!(code, INVALID);
        assert_eq!(
            message,
            "Общий CSS, строка 2: картинка не найдена, загрузите её заново"
        );
        let check: CssCheck = serde_json::from_value(json!({ "css": {
            "page": lost,
            "hero": css,
            "plans": ".ld-card {",
            "cta": "",
            "video": "x"
        } }))
        .unwrap();
        let report = serde_json::to_value(ops.check_css(&check).await.unwrap()).unwrap();
        assert_eq!(
            report["page"],
            json!({ "line": 2, "problem": "картинка не найдена, загрузите её заново" })
        );
        assert_eq!(
            report["hero"],
            json!({ "style": format!("[data-block=\"hero\"] h1 {{}}\n[data-block=\"hero\"] {{\n  background: url(\"{}\");\n}}\n", kept.url) })
        );
        assert_eq!(
            report["plans"],
            json!({ "line": 1, "problem": "не закрыта скобка {" })
        );
        assert_eq!(report["cta"], json!({ "style": "" }));
        assert!(report.get("video").is_none());

        ops.update(&update(json!({ "blocks": [], "css": {} })), env.admin)
            .await
            .unwrap();
        assert!(!stored(&env, &kept.tag).await);
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn картинок_для_лендинга_не_больше_сорока() {
        let Some(env) = env().await else { return };
        let ops = env.services.landing();
        let mut tags = Vec::new();
        for fill in 0..STORED_IMAGES_MAX {
            let fill = u8::try_from(fill).unwrap();
            tags.push(
                ops.upload_image(&png(400, 300, fill), env.admin)
                    .await
                    .unwrap()
                    .tag,
            );
        }
        let (code, message) = rejected(ops.upload_image(&png(400, 300, 200), env.admin).await);
        assert_eq!(code, "landing_image.limit");
        assert!(message.contains("40"), "{message}");
        assert!(ops.upload_image(&png(400, 300, 0), env.admin).await.is_ok());
        ops.update(
            &update(json!({ "blocks": [
                { "kind": "hero", "art": "image", "image": { "tag": tags[1] } }
            ] })),
            env.admin,
        )
        .await
        .unwrap();
        let old: Vec<&str> = tags[1..4].iter().map(String::as_str).collect();
        age(&env, &old).await;
        let added = ops
            .upload_image(&png(400, 300, 200), env.admin)
            .await
            .unwrap();
        assert!(stored(&env, &added.tag).await);
        assert!(stored(&env, &tags[1]).await);
        assert!(!stored(&env, &tags[2]).await);
        assert!(!stored(&env, &tags[3]).await);
        let mut conn = env.db.store.conn().await.unwrap();
        assert_eq!(
            conn.site_assets().count_prefix(ASSET_PREFIX).await.unwrap(),
            39
        );
        drop(conn);
        env.db.cleanup().await;
    }
}
