use crate::css::{CssSite, Place, PlacedCss};
use crate::landing::BlockKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CssPlace {
    Page,
    Hero,
    Features,
    Steps,
    Plans,
    Servers,
    Faq,
    Cta,
    Footer,
}

impl CssPlace {
    pub const fn block(self) -> Option<BlockKind> {
        match self {
            CssPlace::Page => None,
            CssPlace::Hero => Some(BlockKind::Hero),
            CssPlace::Features => Some(BlockKind::Features),
            CssPlace::Steps => Some(BlockKind::Steps),
            CssPlace::Plans => Some(BlockKind::Plans),
            CssPlace::Servers => Some(BlockKind::Servers),
            CssPlace::Faq => Some(BlockKind::Faq),
            CssPlace::Cta => Some(BlockKind::Cta),
            CssPlace::Footer => Some(BlockKind::Footer),
        }
    }
}

impl Place for CssPlace {
    const SITE: CssSite = CssSite {
        files: "/api/public/landing/",
        classes: &["ld", "ld-phone"],
    };

    const ALL: &'static [CssPlace] = &[
        CssPlace::Page,
        CssPlace::Hero,
        CssPlace::Features,
        CssPlace::Steps,
        CssPlace::Plans,
        CssPlace::Servers,
        CssPlace::Faq,
        CssPlace::Cta,
        CssPlace::Footer,
    ];

    fn as_str(self) -> &'static str {
        self.block().map_or("page", BlockKind::as_str)
    }

    fn scope(self) -> Option<String> {
        self.block()
            .map(|kind| format!("[data-block=\"{}\"]", kind.as_str()))
    }

    fn title(self) -> String {
        match self.block() {
            Some(kind) => format!("{}, CSS блока", kind.title()),
            None => "Общий CSS".to_string(),
        }
    }
}

pub type LandingCss = PlacedCss<CssPlace>;
