use crate::client::Locale;
use crate::image::{ImageKind, ImageRules};
use crate::landing::{clean_line, clean_paragraph};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const APPS_MAX: usize = 12;
pub const NAME_MAX: usize = 40;
pub const LABEL_MAX: usize = 40;
pub const TITLE_MAX: usize = 80;
pub const TEXT_MAX: usize = 500;
pub const LINK_MAX: usize = 2048;
pub const BUTTONS_MAX: usize = 6;
pub const NOTES_MAX: usize = 4;
pub const ICON_TAG_LEN: usize = 16;
pub const ICON_SVG_MAX: usize = 32 * 1024;
pub const URL_SLOT: &str = "{url}";
pub const NAME_SLOT: &str = "{name}";
const SVG_NS: &str = "http://www.w3.org/2000/svg";

pub const ICON_RULES: ImageRules = ImageRules {
    kinds: &[ImageKind::Png, ImageKind::Jpeg, ImageKind::Webp],
    max_bytes: 256 * 1024,
    min_side: 32,
    max_side: 1024,
    square: true,
};

const FORBIDDEN_SCHEMES: [&str; 18] = [
    "javascript",
    "vbscript",
    "data",
    "file",
    "about",
    "blob",
    "filesystem",
    "http",
    "ftp",
    "ws",
    "wss",
    "chrome",
    "chrome-extension",
    "view-source",
    "jar",
    "mailto",
    "tel",
    "sms",
];

const CRYPT_SLOTS: [&str; 3] = ["HAPP_CRYPT3_LINK", "HAPP_CRYPT4_LINK", "INCY_CRYPT1_LINK"];

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Ios,
    Android,
    Windows,
    Macos,
    Linux,
    AndroidTv,
    AppleTv,
}

impl Platform {
    pub const ALL: [Platform; 7] = [
        Platform::Ios,
        Platform::Android,
        Platform::Windows,
        Platform::Macos,
        Platform::Linux,
        Platform::AndroidTv,
        Platform::AppleTv,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Platform::Ios => "iPhone и iPad",
            Platform::Android => "Android",
            Platform::Windows => "Windows",
            Platform::Macos => "macOS",
            Platform::Linux => "Linux",
            Platform::AndroidTv => "Android TV",
            Platform::AppleTv => "Apple TV",
        }
    }

    pub fn remnawave_key(self) -> &'static str {
        match self {
            Platform::Ios => "ios",
            Platform::Android => "android",
            Platform::Windows => "windows",
            Platform::Macos => "macos",
            Platform::Linux => "linux",
            Platform::AndroidTv => "androidTV",
            Platform::AppleTv => "appleTV",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Text {
    #[serde(default)]
    pub ru: String,
    #[serde(default)]
    pub en: String,
}

impl Text {
    pub fn new(ru: &str, en: &str) -> Text {
        Text {
            ru: ru.to_string(),
            en: en.to_string(),
        }
    }

    pub fn get(&self, locale: Locale) -> &str {
        match locale {
            Locale::En if !self.en.is_empty() => &self.en,
            _ if self.ru.is_empty() => &self.en,
            _ => &self.ru,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ru.is_empty() && self.en.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkButton {
    #[serde(default)]
    pub label: Text,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Step {
    Install {
        #[serde(default)]
        text: Text,
        #[serde(default)]
        buttons: Vec<LinkButton>,
    },
    Add {
        #[serde(default)]
        text: Text,
        #[serde(default)]
        link: Option<String>,
    },
    Connect {
        #[serde(default)]
        text: Text,
    },
    Note {
        #[serde(default)]
        title: Text,
        #[serde(default)]
        text: Text,
        #[serde(default)]
        buttons: Vec<LinkButton>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconKind {
    Png,
    Jpeg,
    Webp,
    Svg,
}

impl IconKind {
    pub fn content_type(self) -> &'static str {
        match self {
            IconKind::Png => "image/png",
            IconKind::Jpeg => "image/jpeg",
            IconKind::Webp => "image/webp",
            IconKind::Svg => "image/svg+xml",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            IconKind::Png => "png",
            IconKind::Jpeg => "jpg",
            IconKind::Webp => "webp",
            IconKind::Svg => "svg",
        }
    }

    pub fn from_image(kind: ImageKind) -> Option<IconKind> {
        match kind {
            ImageKind::Png => Some(IconKind::Png),
            ImageKind::Jpeg => Some(IconKind::Jpeg),
            ImageKind::Webp => Some(IconKind::Webp),
            ImageKind::Ico => None,
        }
    }

    pub fn sniff(bytes: &[u8]) -> Option<IconKind> {
        if check_svg(bytes).is_ok() {
            return Some(IconKind::Svg);
        }
        ImageKind::sniff(bytes)
            .filter(|kind| ICON_RULES.kinds.contains(kind))
            .and_then(IconKind::from_image)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppIcon {
    pub tag: String,
    pub kind: IconKind,
}

impl AppIcon {
    pub fn path(&self) -> String {
        format!(
            "/api/public/app-icon/{}.{}",
            self.tag,
            self.kind.extension()
        )
    }
}

pub fn valid_icon_tag(tag: &str) -> bool {
    tag.len() == ICON_TAG_LEN
        && tag
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectApp {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<AppIcon>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub steps: Vec<Step>,
}

impl ConnectApp {
    pub fn add_link(&self) -> Option<&str> {
        self.steps.iter().find_map(|step| match step {
            Step::Add { link, .. } => link.as_deref(),
            _ => None,
        })
    }

    pub fn install_buttons(&self) -> &[LinkButton] {
        self.steps
            .iter()
            .find_map(|step| match step {
                Step::Install { buttons, .. } => Some(buttons.as_slice()),
                _ => None,
            })
            .unwrap_or(&[])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkSpot {
    pub platform: Platform,
    pub app: String,
    pub step: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConnectApps(pub BTreeMap<Platform, Vec<ConnectApp>>);

impl ConnectApps {
    pub fn standard() -> ConnectApps {
        let add = Text::new(
            "Одна кнопка, ссылка откроется в приложении",
            "One click opens the link in the app",
        );
        let app = |name: &str, store: Text, button: Text, url: &str, link: &str| ConnectApp {
            id: name.to_ascii_lowercase(),
            name: name.to_string(),
            icon: None,
            enabled: true,
            steps: vec![
                Step::Install {
                    text: store,
                    buttons: vec![LinkButton {
                        label: button,
                        url: url.to_string(),
                    }],
                },
                Step::Add {
                    text: add.clone(),
                    link: Some(link.to_string()),
                },
                Step::Connect {
                    text: Text::new(
                        &format!("Нажмите кнопку подключения в {name}, сервер выберется сам"),
                        &format!("Click connect in {name}, and the server is picked automatically"),
                    ),
                },
            ],
        };
        let happ = |store: Text, button: Text, url: &str| {
            app("Happ", store, button, url, "happ://add/{url}")
        };
        let hiddify = |store: Text| {
            app(
                "Hiddify",
                store,
                Text::new("Скачать", "Download"),
                "https://github.com/hiddify/hiddify-app/releases",
                "hiddify://import/{url}",
            )
        };
        let mut apps = BTreeMap::new();
        apps.insert(
            Platform::Ios,
            vec![happ(
                Text::new("App Store, бесплатно", "App Store, free"),
                Text::new("App Store", "App Store"),
                "https://apps.apple.com/app/happ-proxy-utility/id6504287215",
            )],
        );
        apps.insert(
            Platform::Android,
            vec![happ(
                Text::new("Google Play или файл APK", "Google Play or an APK file"),
                Text::new("Google Play", "Google Play"),
                "https://play.google.com/store/apps/details?id=com.happproxy",
            )],
        );
        apps.insert(
            Platform::Windows,
            vec![hiddify(Text::new(
                "Установщик для Windows 10 и 11",
                "Installer for Windows 10 and 11",
            ))],
        );
        apps.insert(
            Platform::Macos,
            vec![hiddify(Text::new(
                "Установщик .dmg для macOS",
                ".dmg installer for macOS",
            ))],
        );
        apps.insert(
            Platform::Linux,
            vec![hiddify(Text::new(
                "Пакеты .deb и AppImage",
                ".deb and AppImage packages",
            ))],
        );
        ConnectApps(apps).full()
    }

    pub fn full(mut self) -> ConnectApps {
        for platform in Platform::ALL {
            self.0.entry(platform).or_default();
        }
        self
    }

    pub fn on(&self, platform: Platform) -> &[ConnectApp] {
        self.0.get(&platform).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn count(&self) -> usize {
        self.0.values().map(Vec::len).sum()
    }

    pub fn shown(&self) -> ConnectApps {
        ConnectApps(
            self.0
                .iter()
                .map(|(platform, apps)| {
                    let list: Vec<ConnectApp> =
                        apps.iter().filter(|app| app.enabled).cloned().collect();
                    (*platform, list)
                })
                .filter(|(_, apps)| !apps.is_empty())
                .collect(),
        )
    }

    pub fn launch_link(&self, spot: &LinkSpot) -> Option<(&ConnectApp, &str)> {
        let app = self
            .on(spot.platform)
            .iter()
            .find(|app| app.enabled && app.id == spot.app)?;
        let link = match (app.steps.get(spot.step)?, spot.button) {
            (Step::Add { link, .. }, None) => link.as_deref()?,
            (Step::Note { buttons, .. }, Some(button)) => buttons.get(button)?.url.as_str(),
            _ => return None,
        };
        app_scheme(link)?;
        Some((app, link))
    }

    pub fn icon_tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = self
            .0
            .values()
            .flatten()
            .filter_map(|app| app.icon.as_ref().map(|icon| icon.tag.clone()))
            .collect();
        tags.sort();
        tags.dedup();
        tags
    }

    pub fn icons(&self) -> Vec<(String, &AppIcon)> {
        self.0
            .iter()
            .flat_map(|(platform, apps)| {
                apps.iter().filter_map(move |app| {
                    app.icon
                        .as_ref()
                        .map(|icon| (format!("{} · {}", platform.title(), app.name), icon))
                })
            })
            .collect()
    }

    pub fn checked(self) -> Result<ConnectApps, String> {
        let mut out = BTreeMap::new();
        for (platform, apps) in self.0 {
            if apps.len() > APPS_MAX {
                return Err(format!(
                    "{}: не больше {APPS_MAX} приложений",
                    platform.title()
                ));
            }
            let mut list: Vec<ConnectApp> = Vec::with_capacity(apps.len());
            for (index, app) in apps.into_iter().enumerate() {
                let label = match clean_line(&app.name) {
                    name if name.is_empty() => format!("приложение {}", index + 1),
                    name => name,
                };
                let mut app = check_app(app)
                    .map_err(|reason| format!("{} · {label}: {reason}", platform.title()))?;
                app.id = unique_id(&app.name, &list);
                list.push(app);
            }
            out.insert(platform, list);
        }
        Ok(ConnectApps(out).full())
    }
}

fn unique_id(name: &str, taken: &[ConnectApp]) -> String {
    let mut base = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            base.push(c);
        } else if !base.is_empty() && !base.ends_with('-') {
            base.push('-');
        }
    }
    let mut base: String = base.trim_matches('-').chars().take(32).collect();
    if base.is_empty() {
        base = "app".to_string();
    }
    let free = |id: &str| !taken.iter().any(|app| app.id == id);
    if free(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| free(id))
        .unwrap_or(base)
}

fn too_long(text: &str, max: usize) -> bool {
    text.chars().count() > max
}

fn check_app(app: ConnectApp) -> Result<ConnectApp, String> {
    let name = clean_line(&app.name);
    if name.is_empty() {
        return Err("напишите название".to_string());
    }
    if too_long(&name, NAME_MAX) {
        return Err(format!("название длиннее {NAME_MAX} символов"));
    }
    if app
        .icon
        .as_ref()
        .is_some_and(|icon| !valid_icon_tag(&icon.tag))
    {
        return Err("значок испорчен, загрузите его заново".to_string());
    }
    Ok(ConnectApp {
        id: String::new(),
        name,
        icon: app.icon,
        enabled: app.enabled,
        steps: check_steps(app.steps)?,
    })
}

fn check_steps(steps: Vec<Step>) -> Result<Vec<Step>, String> {
    let mut out = Vec::with_capacity(steps.len());
    let mut places = [None::<usize>; 3];
    let mut notes = 0;
    for step in steps {
        let at = out.len();
        let checked = match step {
            Step::Install { text, buttons } => {
                if places[0].replace(at).is_some() {
                    return Err("шаг установки повторяется".to_string());
                }
                Step::Install {
                    text: paragraph(text, TEXT_MAX, "текст шага установки")?,
                    buttons: check_buttons(buttons, true)?,
                }
            }
            Step::Add { text, link } => {
                if places[1].replace(at).is_some() {
                    return Err("шаг добавления повторяется".to_string());
                }
                let link = match link.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
                    Some(link) => Some(
                        check_add_link(link)
                            .map_err(|problem| format!("ссылка добавления: {}", problem.text()))?,
                    ),
                    None => None,
                };
                Step::Add {
                    text: paragraph(text, TEXT_MAX, "текст шага добавления")?,
                    link,
                }
            }
            Step::Connect { text } => {
                if places[2].replace(at).is_some() {
                    return Err("шаг включения повторяется".to_string());
                }
                Step::Connect {
                    text: paragraph(text, TEXT_MAX, "текст шага включения")?,
                }
            }
            Step::Note {
                title,
                text,
                buttons,
            } => {
                notes += 1;
                if notes > NOTES_MAX {
                    return Err(format!("не больше {NOTES_MAX} дополнительных шагов"));
                }
                let title = line(title, TITLE_MAX, "заголовок шага")?;
                if title.is_empty() {
                    return Err("у дополнительного шага нет заголовка".to_string());
                }
                Step::Note {
                    title,
                    text: paragraph(text, TEXT_MAX, "текст дополнительного шага")?,
                    buttons: check_buttons(buttons, false)?,
                }
            }
        };
        out.push(checked);
    }
    match places {
        [None, _, _] => Err("нет шага установки".to_string()),
        [_, None, _] => Err("нет шага добавления подписки".to_string()),
        [_, _, None] => Err("нет шага включения".to_string()),
        [Some(install), Some(add), Some(connect)] if install < add && add < connect => Ok(out),
        _ => Err("шаги установки, добавления и включения идут не по порядку".to_string()),
    }
}

fn line(text: Text, max: usize, what: &str) -> Result<Text, String> {
    let text = Text {
        ru: clean_line(&text.ru),
        en: clean_line(&text.en),
    };
    if too_long(&text.ru, max) || too_long(&text.en, max) {
        return Err(format!("{what} длиннее {max} символов"));
    }
    Ok(text)
}

fn paragraph(text: Text, max: usize, what: &str) -> Result<Text, String> {
    let text = Text {
        ru: clean_paragraph(&text.ru),
        en: clean_paragraph(&text.en),
    };
    if too_long(&text.ru, max) || too_long(&text.en, max) {
        return Err(format!("{what} длиннее {max} символов"));
    }
    Ok(text)
}

fn check_buttons(buttons: Vec<LinkButton>, install: bool) -> Result<Vec<LinkButton>, String> {
    if buttons.len() > BUTTONS_MAX {
        return Err(format!("не больше {BUTTONS_MAX} кнопок в шаге"));
    }
    buttons
        .into_iter()
        .map(|button| {
            let label = line(button.label, LABEL_MAX, "надпись кнопки")?;
            if label.is_empty() {
                return Err("у кнопки нет надписи".to_string());
            }
            let url = if install {
                check_https(&button.url)
            } else {
                check_note_link(&button.url)
            }
            .map_err(|problem| format!("кнопка «{}»: {}", label.get(Locale::Ru), problem.text()))?;
            Ok(LinkButton { label, url })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkProblem {
    Empty,
    TooLong,
    Spaces,
    NotHttps,
    Scheme,
    Forbidden,
    NoUrl,
    UnknownSlot,
}

impl LinkProblem {
    pub fn text(self) -> &'static str {
        match self {
            LinkProblem::Empty => "нужна ссылка",
            LinkProblem::TooLong => "ссылка слишком длинная",
            LinkProblem::Spaces => "в ссылке есть пробелы, кавычки или угловые скобки",
            LinkProblem::NotHttps => "нужна ссылка https://",
            LinkProblem::Scheme => "нужна схема приложения вроде happ:// или ссылка https://",
            LinkProblem::Forbidden => "такие ссылки запрещены, нужна схема приложения или https://",
            LinkProblem::NoUrl => "нет подстановки {url}: без неё приложение не получит подписку",
            LinkProblem::UnknownSlot => "подставить можно только {url} и {name}",
        }
    }
}

fn bad_char(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>' | '\\' | '`')
}

pub fn check_https(raw: &str) -> Result<String, LinkProblem> {
    let link = raw.trim();
    if link.is_empty() {
        return Err(LinkProblem::Empty);
    }
    if link.len() > LINK_MAX {
        return Err(LinkProblem::TooLong);
    }
    if link.chars().any(bad_char) || link.contains(['{', '}']) {
        return Err(LinkProblem::Spaces);
    }
    let scheme_len = "https://".len();
    if !link
        .get(..scheme_len)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
    {
        return Err(LinkProblem::NotHttps);
    }
    let rest = &link[scheme_len..];
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit('@').next().unwrap_or_default();
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']' | '_'))
    {
        return Err(LinkProblem::NotHttps);
    }
    Ok(format!("https://{rest}"))
}

fn app_link(raw: &str, need_url: bool) -> Result<String, LinkProblem> {
    let link = raw.trim();
    if link.is_empty() {
        return Err(LinkProblem::Empty);
    }
    if link.len() > LINK_MAX {
        return Err(LinkProblem::TooLong);
    }
    let probe = link.replace(URL_SLOT, "u").replace(NAME_SLOT, "n");
    if probe.contains(['{', '}']) {
        return Err(LinkProblem::UnknownSlot);
    }
    if probe.chars().any(bad_char) {
        return Err(LinkProblem::Spaces);
    }
    let Some((scheme, rest)) = probe.split_once(':') else {
        return Err(LinkProblem::Scheme);
    };
    let scheme = scheme.to_ascii_lowercase();
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '-' | '.'))
        && (2..=32).contains(&scheme.len());
    if !valid || rest.is_empty() {
        return Err(LinkProblem::Scheme);
    }
    if FORBIDDEN_SCHEMES.contains(&scheme.as_str()) {
        return Err(LinkProblem::Forbidden);
    }
    if scheme == "https" {
        check_https(&probe)?;
    }
    if need_url && !link.contains(URL_SLOT) {
        return Err(LinkProblem::NoUrl);
    }
    Ok(link.to_string())
}

pub fn check_add_link(raw: &str) -> Result<String, LinkProblem> {
    app_link(raw, true)
}

pub fn check_note_link(raw: &str) -> Result<String, LinkProblem> {
    app_link(raw, false)
}

pub fn app_scheme(link: &str) -> Option<String> {
    let link = check_note_link(link).ok()?;
    let (scheme, _) = link.split_once(':')?;
    let scheme = scheme.to_ascii_lowercase();
    (scheme != "https" && !scheme.contains(['{', '}'])).then_some(scheme)
}

pub fn fill_link(link: &str, url: &str, name: &str) -> String {
    let name: String = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    link.replace(URL_SLOT, url).replace(NAME_SLOT, &name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SvgProblem {
    TooHeavy,
    Format,
    Unsafe,
}

impl SvgProblem {
    pub fn text(self) -> &'static str {
        match self {
            SvgProblem::TooHeavy => "файл SVG тяжелее 32 КБ",
            SvgProblem::Format => "это не картинка SVG",
            SvgProblem::Unsafe => "в SVG есть скрипты или внешние ссылки",
        }
    }
}

fn svg_body(lower: &str) -> Option<&str> {
    let mut rest = lower.trim_start_matches('\u{feff}').trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("<?xml") {
            rest = after.split_once("?>")?.1.trim_start();
        } else if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.split_once("-->")?.1.trim_start();
        } else {
            break;
        }
    }
    let after = rest.strip_prefix("<svg")?;
    after
        .starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .then_some(rest)
}

fn event_attribute(lower: &str) -> bool {
    let bytes = lower.as_bytes();
    (1..bytes.len().saturating_sub(2)).any(|i| {
        if bytes[i] != b'o' || bytes[i + 1] != b'n' {
            return false;
        }
        if !(bytes[i - 1].is_ascii_whitespace() || matches!(bytes[i - 1], b'"' | b'\'' | b'/')) {
            return false;
        }
        let mut at = i + 2;
        while at < bytes.len() && bytes[at].is_ascii_alphabetic() {
            at += 1;
        }
        if at == i + 2 {
            return false;
        }
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        bytes.get(at) == Some(&b'=')
    })
}

fn outside_reference(lower: &str) -> bool {
    let local = |value: &str| {
        let value = value
            .trim_start()
            .trim_start_matches(['"', '\''])
            .trim_start();
        value.starts_with('#') || value.starts_with("data:image/")
    };
    let href = lower.match_indices("href").any(|(at, _)| {
        lower[at + 4..]
            .trim_start()
            .strip_prefix('=')
            .is_some_and(|value| !local(value))
    });
    href || lower
        .match_indices("url(")
        .any(|(at, _)| !local(&lower[at + 4..]))
}

fn prefixed_element(lower: &str) -> bool {
    lower.match_indices('<').any(|(at, _)| {
        lower[at + 1..]
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || matches!(c, '>' | '/'))
            .next()
            .is_some_and(|name| name.contains(':'))
    })
}

pub fn check_svg(bytes: &[u8]) -> Result<(), SvgProblem> {
    if bytes.len() > ICON_SVG_MAX {
        return Err(SvgProblem::TooHeavy);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| SvgProblem::Format)?;
    let lower = text.to_ascii_lowercase();
    if svg_body(&lower).is_none() || !lower.contains("</svg>") {
        return Err(SvgProblem::Format);
    }
    let unsafe_parts = [
        "<script",
        "<foreignobject",
        "<iframe",
        "<embed",
        "<object",
        "<!doctype",
        "<!entity",
        "<handler",
        "<listener",
        "javascript:",
        "vbscript:",
        "@import",
    ];
    if unsafe_parts.iter().any(|part| lower.contains(part))
        || prefixed_element(&lower)
        || event_attribute(&lower)
        || outside_reference(&lower)
    {
        return Err(SvgProblem::Unsafe);
    }
    Ok(())
}

pub fn with_svg_namespace(svg: &str) -> String {
    let lower = svg.to_ascii_lowercase();
    let Some(body) = svg_body(&lower) else {
        return svg.to_string();
    };
    let start = lower.len() - body.len();
    let head_end = body.find('>').map_or(body.len(), |end| end);
    if body[..head_end].contains("xmlns=") && body[..head_end].contains(SVG_NS) {
        return svg.to_string();
    }
    let insert = start + "<svg".len();
    format!("{} xmlns=\"{SVG_NS}\"{}", &svg[..insert], &svg[insert..])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedApp {
    pub platform: Platform,
    pub app: ConnectApp,
    pub svg: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemnaImport {
    pub apps: Vec<ImportedApp>,
    pub notes: Vec<String>,
}

#[derive(Default)]
struct ImportLog {
    other_locales: bool,
    notes: usize,
    note_example: Option<String>,
    username: bool,
    crypt: Vec<String>,
    dropped_buttons: Vec<String>,
    no_link: Vec<String>,
    bad_link: Vec<String>,
    bad_icons: usize,
    many_notes: Vec<String>,
    featured: Vec<String>,
    over_limit: Vec<String>,
    unknown_platforms: Vec<String>,
}

fn remember(list: &mut Vec<String>, name: &str) {
    if !list.iter().any(|known| known == name) {
        list.push(name.to_string());
    }
}

fn cut(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        return text;
    }
    let mut short: String = text.chars().take(max.saturating_sub(1)).collect();
    short = short.trim_end().to_string();
    short.push('…');
    short
}

fn remna_text(value: Option<&Value>, max: usize, single: bool) -> Text {
    let clean = |raw: &str| {
        let text = if single {
            clean_line(raw)
        } else {
            clean_paragraph(raw)
        };
        cut(text, max)
    };
    let get = |lang: &str| {
        value
            .and_then(|v| v.get(lang))
            .and_then(Value::as_str)
            .map(clean)
            .filter(|text| !text.is_empty())
    };
    let en = get("en");
    let ru = get("ru").or_else(|| {
        value.and_then(Value::as_object).and_then(|map| {
            map.values()
                .filter_map(Value::as_str)
                .map(clean)
                .find(|t| !t.is_empty())
        })
    });
    match (ru, en) {
        (Some(ru), en) => Text {
            ru,
            en: en.unwrap_or_default(),
        },
        (None, Some(en)) => Text {
            ru: en,
            en: String::new(),
        },
        (None, None) => Text::default(),
    }
}

fn remna_link(raw: &str, app: &str, log: &mut ImportLog) -> Option<String> {
    let mut link = raw.trim().to_string();
    for slot in CRYPT_SLOTS {
        let marker = format!("{{{{{slot}}}}}");
        if link.contains(&marker) {
            link = link.replace(&marker, URL_SLOT);
            remember(&mut log.crypt, app);
        }
    }
    if link.contains("{{USERNAME}}") {
        link = link.replace("{{USERNAME}}", NAME_SLOT);
        log.username = true;
    }
    link = link.replace("{{SUBSCRIPTION_LINK}}", URL_SLOT);
    if link.contains("{{") {
        return None;
    }
    Some(link)
}

fn buttons_of(block: &Value) -> Vec<&Value> {
    block
        .get("buttons")
        .and_then(Value::as_array)
        .map(|list| list.iter().collect())
        .unwrap_or_default()
}

fn kind_of(button: &Value) -> &str {
    button
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn remna_buttons(block: &Value, install: bool, app: &str, log: &mut ImportLog) -> Vec<LinkButton> {
    let mut out = Vec::new();
    for button in buttons_of(block) {
        if kind_of(button) != "external" {
            continue;
        }
        let label = remna_text(button.get("text"), LABEL_MAX, true);
        let raw = button
            .get("link")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let url = remna_link(raw, app, log).and_then(|link| {
            if install {
                check_https(&link).ok()
            } else {
                check_note_link(&link).ok()
            }
        });
        match url {
            Some(url) if !label.is_empty() && out.len() < BUTTONS_MAX => {
                out.push(LinkButton { label, url });
            }
            _ => remember(&mut log.dropped_buttons, app),
        }
    }
    out
}

fn remna_steps(app: &str, blocks: &[&Value], log: &mut ImportLog) -> Vec<Step> {
    let add_at = blocks.iter().position(|block| {
        buttons_of(block)
            .iter()
            .any(|button| kind_of(button) == "subscriptionLink")
    });
    let before = add_at.unwrap_or(blocks.len());
    let install_at = (0..before)
        .find(|&i| {
            buttons_of(blocks[i])
                .iter()
                .any(|button| kind_of(button) == "external")
        })
        .or_else(|| (before > 0).then_some(0));
    let connect_at = blocks
        .len()
        .checked_sub(1)
        .filter(|&last| Some(last) != install_at && add_at.is_none_or(|add| last > add));
    let mut steps = Vec::with_capacity(blocks.len() + 3);
    let mut notes = 0;
    for (i, block) in blocks.iter().enumerate() {
        let text = remna_text(block.get("description"), TEXT_MAX, false);
        if Some(i) == install_at {
            steps.push(Step::Install {
                text,
                buttons: remna_buttons(block, true, app, log),
            });
        } else if Some(i) == add_at {
            let raw = buttons_of(block)
                .into_iter()
                .find(|button| kind_of(button) == "subscriptionLink")
                .and_then(|button| button.get("link"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let link = remna_link(raw, app, log).and_then(|link| check_add_link(&link).ok());
            if link.is_none() {
                remember(&mut log.bad_link, app);
            }
            steps.push(Step::Add { text, link });
        } else if Some(i) == connect_at {
            steps.push(Step::Connect { text });
        } else {
            if notes == NOTES_MAX {
                remember(&mut log.many_notes, app);
                continue;
            }
            let title = remna_text(block.get("title"), TITLE_MAX, true);
            if title.is_empty() {
                continue;
            }
            notes += 1;
            log.notes += 1;
            if log.note_example.is_none() {
                log.note_example = Some(title.ru.clone());
            }
            steps.push(Step::Note {
                title,
                text,
                buttons: remna_buttons(block, false, app, log),
            });
        }
    }
    if install_at.is_none() {
        steps.insert(
            0,
            Step::Install {
                text: Text::default(),
                buttons: Vec::new(),
            },
        );
    }
    if add_at.is_none() {
        remember(&mut log.no_link, app);
        let at = steps
            .iter()
            .position(|step| matches!(step, Step::Install { .. }))
            .map_or(0, |at| at + 1);
        steps.insert(
            at,
            Step::Add {
                text: Text::default(),
                link: None,
            },
        );
    }
    if connect_at.is_none() {
        steps.push(Step::Connect {
            text: Text::default(),
        });
    }
    steps
}

fn list(names: &[String]) -> String {
    names.join(", ")
}

impl ImportLog {
    fn notes(self) -> Vec<String> {
        let mut out = Vec::new();
        if self.other_locales {
            out.push(
                "Тексты переносятся на русском и английском, другие языки не переносятся"
                    .to_string(),
            );
        }
        if self.notes > 0 {
            out.push(match &self.note_example {
                Some(example) => format!(
                    "Дополнительных шагов: {}, например «{example}». Они встанут между основными, как в Remnawave",
                    self.notes
                ),
                None => format!("Дополнительных шагов: {}", self.notes),
            });
        }
        out.extend(self.featured);
        if self.username {
            out.push(
                "В ссылках добавления вместо имени пользователя Remnawave будет название сервиса"
                    .to_string(),
            );
        }
        if !self.crypt.is_empty() {
            out.push(format!(
                "{}: зашифрованная ссылка заменена обычной ссылкой подписки",
                list(&self.crypt)
            ));
        }
        if !self.no_link.is_empty() {
            out.push(format!(
                "{}: нет кнопки добавления подписки, клиент сможет только скопировать ссылку",
                list(&self.no_link)
            ));
        }
        let bad_link: Vec<String> = self
            .bad_link
            .into_iter()
            .filter(|name| !self.no_link.contains(name))
            .collect();
        if !bad_link.is_empty() {
            out.push(format!(
                "{}: ссылка добавления не подошла, клиент сможет только скопировать ссылку",
                list(&bad_link)
            ));
        }
        if !self.dropped_buttons.is_empty() {
            out.push(format!(
                "{}: пропущены кнопки с неподходящими ссылками",
                list(&self.dropped_buttons)
            ));
        }
        if !self.many_notes.is_empty() {
            out.push(format!(
                "{}: перенесены первые {NOTES_MAX} дополнительных шага",
                list(&self.many_notes)
            ));
        }
        if self.bad_icons > 0 {
            out.push(format!(
                "Значков не подошло: {}. Вместо них будет первая буква названия",
                self.bad_icons
            ));
        }
        out.extend(self.over_limit);
        if !self.unknown_platforms.is_empty() {
            out.push(format!(
                "Не перенесены платформы, которых нет в STEALTHNET: {}",
                list(&self.unknown_platforms)
            ));
        }
        out
    }
}

pub fn from_remnawave(config: &Value) -> Result<RemnaImport, String> {
    let empty = || "В оформлении нет приложений: переносить нечего".to_string();
    let platforms = config
        .get("platforms")
        .and_then(Value::as_object)
        .ok_or_else(empty)?;
    let library = config.get("svgLibrary").and_then(Value::as_object);
    let mut log = ImportLog {
        other_locales: config
            .get("locales")
            .and_then(Value::as_array)
            .is_some_and(|locales| {
                locales
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|code| code != "ru" && code != "en")
            }),
        ..ImportLog::default()
    };
    let mut apps = Vec::new();
    for platform in Platform::ALL {
        let Some(entry) = platforms.get(platform.remnawave_key()) else {
            continue;
        };
        let raw_apps = entry
            .get("apps")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut converted: Vec<(bool, ImportedApp)> = Vec::new();
        for raw in raw_apps {
            let name = cut(
                clean_line(raw.get("name").and_then(Value::as_str).unwrap_or_default()),
                NAME_MAX,
            );
            if name.is_empty() {
                continue;
            }
            let featured = raw
                .get("featured")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let svg = raw
                .get("svgIconKey")
                .and_then(Value::as_str)
                .and_then(|key| library.and_then(|lib| lib.get(key)))
                .and_then(Value::as_str);
            let svg = match svg {
                Some(svg) if check_svg(svg.as_bytes()).is_ok() => Some(with_svg_namespace(svg)),
                Some(_) => {
                    log.bad_icons += 1;
                    None
                }
                None => None,
            };
            let blocks: Vec<&Value> = raw
                .get("blocks")
                .and_then(Value::as_array)
                .map(|blocks| blocks.iter().collect())
                .unwrap_or_default();
            let steps = remna_steps(&name, &blocks, &mut log);
            converted.push((
                featured,
                ImportedApp {
                    platform,
                    app: ConnectApp {
                        id: String::new(),
                        name,
                        icon: None,
                        enabled: true,
                        steps,
                    },
                    svg,
                },
            ));
        }
        let featured: Vec<usize> = converted
            .iter()
            .enumerate()
            .filter(|(_, (featured, _))| *featured)
            .map(|(i, _)| i)
            .collect();
        if let Some(&main) = featured.first() {
            if featured.len() > 1 {
                log.featured.push(format!(
                    "{}: в Remnawave главными отмечено несколько приложений ({}), главным станет первое — {}",
                    platform.title(),
                    featured.len(),
                    converted[main].1.app.name
                ));
            }
            let first = converted.remove(main);
            converted.insert(0, first);
        }
        if converted.len() > APPS_MAX {
            log.over_limit.push(format!(
                "{}: перенесены первые {APPS_MAX} приложений из {}",
                platform.title(),
                converted.len()
            ));
            converted.truncate(APPS_MAX);
        }
        apps.extend(converted.into_iter().map(|(_, app)| app));
    }
    for key in platforms.keys() {
        if !Platform::ALL.iter().any(|p| p.remnawave_key() == key) {
            log.unknown_platforms.push(key.clone());
        }
    }
    if apps.is_empty() {
        return Err(empty());
    }
    Ok(RemnaImport {
        apps,
        notes: log.notes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app(name: &str, link: Option<&str>, buttons: Vec<LinkButton>) -> ConnectApp {
        ConnectApp {
            id: String::new(),
            name: name.to_string(),
            icon: None,
            enabled: true,
            steps: vec![
                Step::Install {
                    text: Text::new("Установите", "Install"),
                    buttons,
                },
                Step::Add {
                    text: Text::default(),
                    link: link.map(str::to_string),
                },
                Step::Connect {
                    text: Text::default(),
                },
            ],
        }
    }

    fn button(url: &str) -> LinkButton {
        LinkButton {
            label: Text::new("Скачать", "Download"),
            url: url.to_string(),
        }
    }

    fn one(platform: Platform, apps: Vec<ConnectApp>) -> ConnectApps {
        let mut map = BTreeMap::new();
        map.insert(platform, apps);
        ConnectApps(map)
    }

    #[test]
    fn ссылки_установки_только_https() {
        assert_eq!(
            check_https(" HTTPS://apps.apple.com/app/x ").unwrap(),
            "https://apps.apple.com/app/x"
        );
        assert_eq!(
            check_https("http://example.com"),
            Err(LinkProblem::NotHttps)
        );
        assert_eq!(
            check_https("javascript:alert(1)"),
            Err(LinkProblem::NotHttps)
        );
        assert_eq!(check_https("https://"), Err(LinkProblem::NotHttps));
        assert_eq!(check_https("https:///path"), Err(LinkProblem::NotHttps));
        assert_eq!(
            check_https("https://ex ample.com"),
            Err(LinkProblem::Spaces)
        );
        assert_eq!(
            check_https("https://example.com/\"x"),
            Err(LinkProblem::Spaces)
        );
        assert_eq!(check_https(""), Err(LinkProblem::Empty));
        let long = format!("https://example.com/{}", "a".repeat(LINK_MAX));
        assert_eq!(check_https(&long), Err(LinkProblem::TooLong));
    }

    #[test]
    fn ссылка_добавления_схема_приложения_или_https_с_подстановкой() {
        assert_eq!(
            check_add_link("happ://add/{url}").unwrap(),
            "happ://add/{url}"
        );
        assert!(check_add_link("stash://install-config?url={url}").is_ok());
        assert!(check_add_link("shadowrocket://add/{url}#{name}").is_ok());
        assert!(check_add_link("https://example.com/import?u={url}").is_ok());
        assert_eq!(check_add_link("happ://add/"), Err(LinkProblem::NoUrl));
        assert_eq!(
            check_add_link("javascript:alert({url})"),
            Err(LinkProblem::Forbidden)
        );
        assert_eq!(
            check_add_link("JavaScript:alert({url})"),
            Err(LinkProblem::Forbidden)
        );
        assert_eq!(
            check_add_link("data:text/html,{url}"),
            Err(LinkProblem::Forbidden)
        );
        assert_eq!(
            check_add_link("vbscript:{url}"),
            Err(LinkProblem::Forbidden)
        );
        assert_eq!(
            check_add_link("http://example.com/{url}"),
            Err(LinkProblem::Forbidden)
        );
        assert_eq!(
            check_add_link("happ://add/{token}"),
            Err(LinkProblem::UnknownSlot)
        );
        assert_eq!(check_add_link("{url}"), Err(LinkProblem::Scheme));
        assert_eq!(check_add_link("1app://{url}"), Err(LinkProblem::Scheme));
        assert_eq!(
            check_add_link("happ://add/ {url}"),
            Err(LinkProblem::Spaces)
        );
        assert_eq!(check_add_link("https:///{url}"), Err(LinkProblem::NotHttps));
        assert_eq!(
            check_note_link("streisand://aW1wb3J0"),
            Ok("streisand://aW1wb3J0".into())
        );
        assert_eq!(
            fill_link(
                "shadowrocket://add/{url}#{name}",
                "https://sub.example/x",
                "Тихий VPN"
            ),
            "shadowrocket://add/https://sub.example/x#%D0%A2%D0%B8%D1%85%D0%B8%D0%B9%20VPN"
        );
    }

    #[test]
    fn проверка_приложений_называет_платформу_и_приложение() {
        let bad = one(
            Platform::Ios,
            vec![app(
                "Happ",
                Some("happ://add/{url}"),
                vec![button("http://apps.apple.com")],
            )],
        );
        let err = bad.checked().unwrap_err();
        assert_eq!(
            err,
            "iPhone и iPad · Happ: кнопка «Скачать»: нужна ссылка https://"
        );
        let no_slot = one(
            Platform::Android,
            vec![app("v2rayNG", Some("v2rayng://install-config"), vec![])],
        );
        assert!(
            no_slot
                .checked()
                .unwrap_err()
                .starts_with("Android · v2rayNG: ссылка добавления: нет подстановки {url}")
        );
        let nameless = one(Platform::Linux, vec![app("  ", None, vec![])]);
        assert_eq!(
            nameless.checked().unwrap_err(),
            "Linux · приложение 1: напишите название"
        );
        let many = one(
            Platform::Windows,
            (0..=APPS_MAX)
                .map(|i| app(&format!("App {i}"), None, vec![]))
                .collect(),
        );
        assert_eq!(
            many.checked().unwrap_err(),
            "Windows: не больше 12 приложений"
        );
    }

    #[test]
    fn шаги_нельзя_терять_и_переставлять_основные() {
        let mut swapped = app("Happ", None, vec![]);
        swapped.steps.swap(0, 1);
        assert_eq!(
            one(Platform::Ios, vec![swapped]).checked().unwrap_err(),
            "iPhone и iPad · Happ: шаги установки, добавления и включения идут не по порядку"
        );
        let mut lost = app("Happ", None, vec![]);
        lost.steps.pop();
        assert_eq!(
            one(Platform::Ios, vec![lost]).checked().unwrap_err(),
            "iPhone и iPad · Happ: нет шага включения"
        );
        let mut noted = app("Happ", None, vec![]);
        noted.steps.insert(
            2,
            Step::Note {
                title: Text::new("  Если   подписка не добавилась ", ""),
                text: Text::new("Скопируйте ссылку\r\n\r\n\r\nи вставьте", ""),
                buttons: vec![button("streisand://import/x")],
            },
        );
        let checked = one(Platform::Ios, vec![noted]).checked().unwrap();
        assert_eq!(
            checked.on(Platform::Ios)[0].steps[2],
            Step::Note {
                title: Text::new("Если подписка не добавилась", ""),
                text: Text::new("Скопируйте ссылку\n\nи вставьте", ""),
                buttons: vec![button("streisand://import/x")],
            }
        );
    }

    #[test]
    fn у_приложений_одной_платформы_разные_идентификаторы() {
        let apps = one(
            Platform::Ios,
            vec![
                app("Happ", None, vec![]),
                app("happ", None, vec![]),
                app("Кот VPN", None, vec![]),
                app("Clash Mi", None, vec![]),
            ],
        )
        .checked()
        .unwrap();
        let ids: Vec<&str> = apps
            .on(Platform::Ios)
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(ids, ["happ", "happ-2", "vpn", "clash-mi"]);
        assert_eq!(apps.0.len(), Platform::ALL.len());
    }

    #[test]
    fn по_умолчанию_нынешний_набор_и_он_проходит_проверку() {
        let standard = ConnectApps::standard();
        assert_eq!(standard.clone().checked().unwrap(), standard);
        let names: Vec<(Platform, &str)> = standard
            .0
            .iter()
            .flat_map(|(platform, apps)| apps.iter().map(move |app| (*platform, app.name.as_str())))
            .collect();
        assert_eq!(
            names,
            [
                (Platform::Ios, "Happ"),
                (Platform::Android, "Happ"),
                (Platform::Windows, "Hiddify"),
                (Platform::Macos, "Hiddify"),
                (Platform::Linux, "Hiddify"),
            ]
        );
        assert_eq!(
            standard.on(Platform::Ios)[0].add_link(),
            Some("happ://add/{url}")
        );
        assert_eq!(
            standard.on(Platform::Linux)[0].add_link(),
            Some("hiddify://import/{url}")
        );
        assert!(standard.on(Platform::AppleTv).is_empty());
        assert_eq!(
            standard.on(Platform::Ios)[0].install_buttons()[0].url,
            "https://apps.apple.com/app/happ-proxy-utility/id6504287215"
        );
        assert_eq!(standard.shown().0.len(), 5);
    }

    #[test]
    fn английский_текст_или_русский_если_английского_нет() {
        let both = Text::new("Скачать", "Download");
        assert_eq!(both.get(Locale::En), "Download");
        assert_eq!(both.get(Locale::Ru), "Скачать");
        let russian = Text::new("Скачать", "");
        assert_eq!(russian.get(Locale::En), "Скачать");
        let english = Text::new("", "Download");
        assert_eq!(english.get(Locale::Ru), "Download");
        let standard = ConnectApps::standard();
        let Step::Connect { text } = &standard.on(Platform::Windows)[0].steps[2] else {
            panic!("нет шага включения");
        };
        assert_eq!(
            text.get(Locale::En),
            "Click connect in Hiddify, and the server is picked automatically"
        );
    }

    #[test]
    fn выключенные_приложения_и_пустые_платформы_клиенту_не_видны() {
        let mut off = app("Stash", Some("stash://install-config?url={url}"), vec![]);
        off.enabled = false;
        let apps = one(
            Platform::Ios,
            vec![off, app("Happ", Some("happ://add/{url}"), vec![])],
        )
        .checked()
        .unwrap();
        let shown = apps.shown();
        assert_eq!(shown.0.len(), 1);
        assert_eq!(shown.on(Platform::Ios)[0].name, "Happ");
    }

    #[test]
    fn запускать_можно_только_схемы_приложений() {
        for (link, scheme) in [
            ("happ://add/{url}", "happ"),
            ("Hiddify://import/{url}", "hiddify"),
            ("v2rayng://install-config?url={url}&name={name}", "v2rayng"),
            ("happ://routing/onadd/eyJ", "happ"),
            ("sing-box://import-remote-profile?url={url}", "sing-box"),
            ("intent://add/{url}#Intent;scheme=happ;end", "intent"),
            (" happ://add/{url} ", "happ"),
        ] {
            assert_eq!(app_scheme(link).as_deref(), Some(scheme), "{link}");
        }
        for link in [
            "https://sub.example/{url}",
            "HTTPS://sub.example/{url}",
            "http://sub.example/{url}",
            "javascript:alert(1)",
            "JavaScript:alert({url})",
            "data:text/html,{url}",
            "file:///etc/passwd",
            "blob:https://panel.example/{url}",
            "about:blank",
            "view-source:https://panel.example",
            "//evil.example/{url}",
            "/cabinet",
            "{url}",
            "{url}://add",
            "a{name}://add/{url}",
            "happ",
            "happ:",
            "happ://add/{token}",
            "happ://add/ {url}",
            "",
        ] {
            assert_eq!(app_scheme(link), None, "{link}");
        }
    }

    #[test]
    fn ссылка_запуска_берётся_только_из_списка_приложений() {
        let note = |url: &str| Step::Note {
            title: Text::new("Правила", "Rules"),
            text: Text::default(),
            buttons: vec![button("https://panel.example/help"), button(url)],
        };
        let mut happ = app(
            "Happ",
            Some("happ://add/{url}"),
            vec![button("https://apps.apple.com/app/x")],
        );
        happ.steps.push(note("happ://routing/onadd/eyJ"));
        let mut web = app("Web", Some("https://open.example/add?sub={url}"), vec![]);
        web.steps.push(note("streisand://rules"));
        let mut off = app("Stash", Some("stash://install-config?url={url}"), vec![]);
        off.enabled = false;
        let apps = one(
            Platform::Ios,
            vec![happ, web, off, app("Copy", None, vec![])],
        )
        .checked()
        .unwrap();
        let find = |app: &str, step: usize, button: Option<usize>| {
            apps.launch_link(&LinkSpot {
                platform: Platform::Ios,
                app: app.to_string(),
                step,
                button,
            })
            .map(|(app, link)| (app.name.as_str(), link))
        };
        assert_eq!(find("happ", 1, None), Some(("Happ", "happ://add/{url}")));
        assert_eq!(
            find("happ", 3, Some(1)),
            Some(("Happ", "happ://routing/onadd/eyJ"))
        );
        assert_eq!(find("web", 3, Some(1)), Some(("Web", "streisand://rules")));
        for (id, step, button) in [
            ("happ", 0, None),
            ("happ", 0, Some(0)),
            ("happ", 1, Some(0)),
            ("happ", 2, None),
            ("happ", 3, None),
            ("happ", 3, Some(0)),
            ("happ", 3, Some(2)),
            ("happ", 9, None),
            ("Happ", 1, None),
            ("web", 1, None),
            ("stash", 1, None),
            ("copy", 1, None),
            ("nothing", 1, None),
        ] {
            assert_eq!(find(id, step, button), None, "{id} {step} {button:?}");
        }
        let elsewhere = LinkSpot {
            platform: Platform::Android,
            app: "happ".into(),
            step: 1,
            button: None,
        };
        assert_eq!(apps.launch_link(&elsewhere), None);
        assert!(ConnectApps::standard().launch_link(&elsewhere).is_some());
        assert_eq!(
            serde_json::to_value(&elsewhere).unwrap(),
            json!({ "platform": "android", "app": "happ", "step": 1 })
        );
        let parsed: LinkSpot = serde_json::from_value(
            json!({ "platform": "android_tv", "app": "x", "step": 3, "button": 1 }),
        )
        .unwrap();
        assert_eq!(
            (parsed.platform, parsed.button),
            (Platform::AndroidTv, Some(1))
        );
        for raw in [
            json!({ "platform": "web", "app": "x", "step": 1 }),
            json!({ "platform": "ios", "app": "x", "step": -1 }),
            json!({ "platform": "ios", "step": 1 }),
        ] {
            assert!(
                serde_json::from_value::<LinkSpot>(raw.clone()).is_err(),
                "{raw}"
            );
        }
    }

    #[test]
    fn svg_проверяется_на_скрипты_и_внешние_ссылки() {
        let good = r##"<?xml version="1.0"?><!-- logo --><svg width="50" height="50" viewBox="0 0 50 50"><defs><linearGradient id="g"/></defs><path fill="url(#g)" d="M0 0h50v50z"/><use href="#g"/></svg>"##;
        assert_eq!(check_svg(good.as_bytes()), Ok(()));
        assert_eq!(IconKind::sniff(good.as_bytes()), Some(IconKind::Svg));
        for bad in [
            r#"<svg><script>alert(1)</script></svg>"#,
            r#"<svg onload="alert(1)"><path/></svg>"#,
            r#"<svg><a href="javascript:alert(1)"><path/></a></svg>"#,
            r#"<svg><image href="https://evil.example/x.png"/></svg>"#,
            r#"<svg><path style="fill:url(https://evil.example/a)"/></svg>"#,
            r#"<svg><foreignObject><div/></foreignObject></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:x="http://www.w3.org/2000/svg"><x:script>alert(1)</x:script></svg>"#,
            r#"<svg xmlns:h="http://www.w3.org/1999/xhtml"><h:iframe src="https://evil.example"/></svg>"#,
        ] {
            assert_eq!(check_svg(bad.as_bytes()), Err(SvgProblem::Unsafe), "{bad}");
        }
        let linked = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><defs><path id="p" d="M0 0h1v1z"/></defs><use xlink:href="#p"/></svg>"##;
        assert_eq!(check_svg(linked.as_bytes()), Ok(()));
        assert_eq!(
            check_svg(b"<html><svg></svg></html>"),
            Err(SvgProblem::Format)
        );
        assert_eq!(check_svg(b"<svgfoo></svgfoo>"), Err(SvgProblem::Format));
        assert_eq!(check_svg(&[0xff, 0xfe]), Err(SvgProblem::Format));
        let heavy = format!("<svg>{}</svg>", " ".repeat(ICON_SVG_MAX));
        assert_eq!(check_svg(heavy.as_bytes()), Err(SvgProblem::TooHeavy));
        assert_eq!(
            with_svg_namespace(r#"<svg width="5"><path/></svg>"#),
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="5"><path/></svg>"#
        );
        let with_ns = r#"<svg xmlns="http://www.w3.org/2000/svg"><path/></svg>"#;
        assert_eq!(with_svg_namespace(with_ns), with_ns);
    }

    fn localized(ru: &str, en: &str) -> Value {
        json!({ "ru": ru, "en": en, "zh": "文字" })
    }

    fn remna_sample() -> Value {
        let icon = r##"<svg width="50" height="50" viewBox="0 0 50 50" fill="none"><path d="M1 1h48v48H1z" fill="#FAFAFA"/></svg>"##;
        let install = |ru: &str, links: &[(&str, &str)]| {
            json!({
                "title": localized("Установка приложения", "App installation"),
                "description": localized(ru, "Install the app"),
                "svgIconKey": "Download", "svgIconColor": "cyan",
                "buttons": links.iter().map(|(label, link)| json!({
                    "type": "external", "link": link, "svgIconKey": "Link",
                    "text": localized(label, label),
                })).collect::<Vec<_>>(),
            })
        };
        let add = |link: &str| {
            json!({
                "title": localized("Добавление подписки", "Add subscription"),
                "description": localized("Нажмите кнопку ниже", "Press the button below"),
                "svgIconKey": "Plus", "svgIconColor": "cyan",
                "buttons": [{ "type": "subscriptionLink", "link": link, "svgIconKey": "Plus", "text": localized("Добавить", "Add") }],
            })
        };
        let block = |ru: &str, color: &str| {
            json!({
                "title": localized(ru, ru),
                "description": localized(&format!("{ru}: подробности"), "Details"),
                "svgIconKey": "Gear", "svgIconColor": color, "buttons": [],
            })
        };
        json!({
            "version": "1",
            "locales": ["en", "ru", "zh"],
            "svgLibrary": {
                "Happ": icon,
                "Bad": "<svg onload=\"x()\"></svg>",
                "Download": icon, "Plus": icon, "Gear": icon, "Link": icon,
            },
            "platforms": {
                "ios": { "displayName": localized("iOS", "iOS"), "svgIconKey": "Download", "apps": [
                    { "name": "Stash", "featured": false, "svgIconKey": "Bad", "blocks": [
                        install("Откройте App Store", &[("App Store", "https://apps.apple.com/app/stash/id1")]),
                        add("stash://install-config?url={{SUBSCRIPTION_LINK}}"),
                        block("Подключение", "teal"),
                    ]},
                    { "name": "Happ", "featured": true, "svgIconKey": "Happ", "blocks": [
                        install("Откройте App Store и установите", &[
                            ("App Store (RU)", "https://apps.apple.com/ru/app/happ/id2"),
                            ("Сайт", "http://happ.example"),
                        ]),
                        add("happ://add/{{HAPP_CRYPT4_LINK}}"),
                        block("Подключение", "teal"),
                    ]},
                ]},
                "windows": { "displayName": localized("Windows", "Windows"), "svgIconKey": "Download", "apps": [
                    { "name": "FlClashX", "featured": true, "blocks": [
                        install("Скачайте установщик", &[("Установщик", "https://github.com/x/setup.exe")]),
                        block("Предупреждение", "red"),
                        add("flclashx://install-config?url={{SUBSCRIPTION_LINK}}"),
                        block("Если подписка не добавилась", "cyan"),
                        block("Подключение", "teal"),
                    ]},
                    { "name": "Koala Clash", "featured": true, "blocks": [
                        install("Скачайте", &[("Установщик", "https://github.com/y/setup.exe")]),
                        add("clashmeta://install-config?name={{USERNAME}}&url={{SUBSCRIPTION_LINK}}"),
                        block("Подключение", "teal"),
                    ]},
                ]},
                "androidTV": { "displayName": localized("TV", "TV"), "svgIconKey": "Download", "apps": [
                    { "name": "vpn4tv", "featured": false, "blocks": [
                        install("Установите", &[("Google Play", "https://play.google.com/store/apps/details?id=tv")]),
                        block("Подключение", "teal"),
                    ]},
                ]},
                "pc": { "apps": [] },
            },
        })
    }

    #[test]
    fn перенос_из_оформления_remnawave() {
        let imported = from_remnawave(&remna_sample()).unwrap();
        let names: Vec<(Platform, &str)> = imported
            .apps
            .iter()
            .map(|a| (a.platform, a.app.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (Platform::Ios, "Happ"),
                (Platform::Ios, "Stash"),
                (Platform::Windows, "FlClashX"),
                (Platform::Windows, "Koala Clash"),
                (Platform::AndroidTv, "vpn4tv"),
            ]
        );
        let happ = &imported.apps[0];
        assert!(
            happ.svg
                .as_deref()
                .unwrap()
                .starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\"")
        );
        assert_eq!(happ.app.add_link(), Some("happ://add/{url}"));
        assert_eq!(happ.app.install_buttons().len(), 1);
        assert_eq!(
            happ.app.install_buttons()[0].label,
            Text::new("App Store (RU)", "App Store (RU)")
        );
        let Step::Install { text, .. } = &happ.app.steps[0] else {
            panic!("первый шаг не установка");
        };
        assert_eq!(
            text,
            &Text::new("Откройте App Store и установите", "Install the app")
        );
        assert_eq!(imported.apps[1].svg, None);
        let flclash = &imported.apps[2].app;
        let kinds: Vec<&str> = flclash
            .steps
            .iter()
            .map(|step| match step {
                Step::Install { .. } => "install",
                Step::Add { .. } => "add",
                Step::Connect { .. } => "connect",
                Step::Note { .. } => "note",
            })
            .collect();
        assert_eq!(kinds, ["install", "note", "add", "note", "connect"]);
        let Step::Note { title, .. } = &flclash.steps[1] else {
            panic!("второй шаг не дополнительный");
        };
        assert_eq!(title.ru, "Предупреждение");
        assert_eq!(
            imported.apps[3].app.add_link(),
            Some("clashmeta://install-config?name={name}&url={url}")
        );
        let tv = &imported.apps[4].app;
        assert_eq!(tv.add_link(), None);
        assert!(matches!(tv.steps[1], Step::Add { link: None, .. }));
        assert!(matches!(tv.steps[2], Step::Connect { .. }));
        let mut map: BTreeMap<Platform, Vec<ConnectApp>> = BTreeMap::new();
        for item in &imported.apps {
            map.entry(item.platform).or_default().push(item.app.clone());
        }
        assert!(ConnectApps(map).checked().is_ok());
        let notes = imported.notes.join("\n");
        for part in [
            "Тексты переносятся на русском и английском",
            "Дополнительных шагов: 2, например «Предупреждение»",
            "Windows: в Remnawave главными отмечено несколько приложений (2), главным станет первое — FlClashX",
            "вместо имени пользователя Remnawave будет название сервиса",
            "Happ: зашифрованная ссылка заменена обычной ссылкой подписки",
            "vpn4tv: нет кнопки добавления подписки",
            "Happ: пропущены кнопки с неподходящими ссылками",
            "Значков не подошло: 1",
            "платформы, которых нет в STEALTHNET: pc",
        ] {
            assert!(notes.contains(part), "нет «{part}» в\n{notes}");
        }
    }

    #[test]
    fn пустое_оформление_не_переносится() {
        assert!(from_remnawave(&json!({ "version": "1" })).is_err());
        assert!(from_remnawave(&json!({ "platforms": { "ios": { "apps": [] } } })).is_err());
    }
}
