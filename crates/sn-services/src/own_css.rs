use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sn_domain::css::{self, CssError, Place, PlacedCss};
use sn_store::Db;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const VERSION_LEN: usize = 16;

pub type CssPlaces = BTreeMap<String, Option<String>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleFile {
    pub version: String,
    pub text: String,
    files: &'static str,
}

impl StyleFile {
    pub fn of<P: Place>(css: &PlacedCss<P>) -> Option<Arc<StyleFile>> {
        let text = css.style();
        (!text.is_empty()).then(|| {
            Arc::new(StyleFile {
                version: short_hash(text.as_bytes()),
                text,
                files: P::SITE.files,
            })
        })
    }

    pub fn path(&self) -> String {
        format!("{}{}.css", self.files, self.version)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CssReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

impl From<CssError> for CssReport {
    fn from(err: CssError) -> CssReport {
        CssReport {
            style: None,
            line: err.line,
            problem: Some(err.reason),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CssCheck {
    #[serde(default)]
    pub css: CssPlaces,
}

pub fn short_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))[..VERSION_LEN].to_string()
}

fn lost_image() -> String {
    "картинка не найдена, загрузите её заново".to_string()
}

pub struct OwnCss<'a> {
    services: &'a Services,
    asset_prefix: &'static str,
    invalid: &'static str,
}

impl<'a> OwnCss<'a> {
    pub fn new(
        services: &'a Services,
        asset_prefix: &'static str,
        invalid: &'static str,
    ) -> OwnCss<'a> {
        OwnCss {
            services,
            asset_prefix,
            invalid,
        }
    }

    async fn missing(&self, tags: BTreeSet<String>) -> ServiceResult<BTreeSet<String>> {
        if tags.is_empty() {
            return Ok(tags);
        }
        let asset_name = |tag: &String| format!("{}{tag}", self.asset_prefix);
        let names: Vec<String> = tags.iter().map(asset_name).collect();
        let mut conn = self.services.store().conn().await?;
        let found: BTreeSet<String> = conn
            .site_assets()
            .existing(&names)
            .await?
            .into_iter()
            .collect();
        Ok(tags
            .into_iter()
            .filter(|tag| !found.contains(&asset_name(tag)))
            .collect())
    }

    pub async fn check<P: Place>(&self, check: &CssCheck) -> ServiceResult<BTreeMap<P, CssReport>> {
        let mut checked = BTreeMap::new();
        for (name, text) in &check.css {
            if let Some(place) = P::parse(name) {
                let verdict = css::check(place, text.as_deref().unwrap_or_default());
                checked.insert(place, verdict);
            }
        }
        let tags = checked
            .values()
            .flatten()
            .flat_map(|css| css.images.keys().cloned())
            .collect();
        let missing = self.missing(tags).await?;
        Ok(checked
            .into_iter()
            .map(|(place, verdict)| {
                let report = match verdict {
                    Err(err) => CssReport::from(err),
                    Ok(css) => match css.images.iter().find(|(tag, _)| missing.contains(*tag)) {
                        Some((_, line)) => CssReport {
                            style: None,
                            line: Some(*line),
                            problem: Some(lost_image()),
                        },
                        None => CssReport {
                            style: Some(css.style),
                            ..CssReport::default()
                        },
                    },
                };
                (place, report)
            })
            .collect())
    }

    pub async fn parse<P: Place>(&self, places: &CssPlaces) -> ServiceResult<PlacedCss<P>> {
        let css = PlacedCss::<P>::parse(places)
            .map_err(|err| ServiceError::rejected(self.invalid, err.to_string()))?;
        let images = css.images();
        let missing = self
            .missing(images.iter().map(|(_, _, tag)| tag.clone()).collect())
            .await?;
        if let Some((place, line, _)) = images.iter().find(|(_, _, tag)| missing.contains(tag)) {
            return Err(ServiceError::rejected(
                self.invalid,
                format!("{}, строка {line}: {}", place.title(), lost_image()),
            ));
        }
        Ok(css)
    }
}
