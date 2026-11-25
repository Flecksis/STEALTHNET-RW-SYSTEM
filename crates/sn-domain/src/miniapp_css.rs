use crate::css::{CssSite, Place, PlacedCss};
use crate::miniapp::HomeKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CssPlace {
    Page,
    Header,
    Banners,
    Greeting,
    Subscription,
    Actions,
    Sections,
    Tabs,
    Profile,
}

impl CssPlace {
    pub const fn block(self) -> Option<HomeKind> {
        match self {
            CssPlace::Banners => Some(HomeKind::Banners),
            CssPlace::Greeting => Some(HomeKind::Greeting),
            CssPlace::Subscription => Some(HomeKind::Subscription),
            CssPlace::Actions => Some(HomeKind::Actions),
            CssPlace::Sections => Some(HomeKind::Sections),
            CssPlace::Page | CssPlace::Header | CssPlace::Tabs | CssPlace::Profile => None,
        }
    }
}

impl Place for CssPlace {
    const SITE: CssSite = CssSite {
        files: "/api/public/miniapp/",
        classes: &["mn"],
    };

    const ALL: &'static [CssPlace] = &[
        CssPlace::Page,
        CssPlace::Header,
        CssPlace::Banners,
        CssPlace::Greeting,
        CssPlace::Subscription,
        CssPlace::Actions,
        CssPlace::Sections,
        CssPlace::Tabs,
        CssPlace::Profile,
    ];

    fn as_str(self) -> &'static str {
        match self {
            CssPlace::Page => "page",
            CssPlace::Header => "header",
            CssPlace::Banners => "banners",
            CssPlace::Greeting => "greeting",
            CssPlace::Subscription => "subscription",
            CssPlace::Actions => "actions",
            CssPlace::Sections => "sections",
            CssPlace::Tabs => "tabs",
            CssPlace::Profile => "profile",
        }
    }

    fn scope(self) -> Option<String> {
        (self != CssPlace::Page).then(|| format!("[data-part=\"{}\"]", self.as_str()))
    }

    fn title(self) -> String {
        match (self, self.block()) {
            (_, Some(kind)) => format!("{}, CSS блока", kind.title()),
            (CssPlace::Header, _) => "Шапка, CSS блока".to_string(),
            (CssPlace::Tabs, _) => "CSS меню внизу".to_string(),
            (CssPlace::Profile, _) => "CSS профиля".to_string(),
            _ => "Общий CSS".to_string(),
        }
    }
}

pub type MiniCss = PlacedCss<CssPlace>;
