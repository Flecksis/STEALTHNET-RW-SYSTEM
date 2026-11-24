use crate::image::ImageKind;
use crate::landing_css::{BLOCK_CSS_MAX, LandingCss, PAGE_CSS_MAX};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Display;

pub const TITLE_MAX: usize = 120;
pub const TEXT_MAX: usize = 600;
pub const ANSWER_MAX: usize = 1500;
pub const NOTE_MAX: usize = 200;
pub const PLANS_TEXT_MAX: usize = 300;
pub const LABEL_MAX: usize = 40;
pub const MENU_MAX: usize = 40;
pub const APP_MAX: usize = 40;
pub const FOOTER_TEXT_MAX: usize = 200;
pub const URL_MAX: usize = 500;
pub const FEATURES_MAX: usize = 6;
pub const STEPS_MAX: usize = 6;
pub const FAQ_MAX: usize = 20;
pub const APPS_MAX: usize = 12;
pub const HERO_BUTTONS_MAX: usize = 2;
pub const CTA_BUTTONS_MAX: usize = 2;
pub const FOOTER_LINKS_MAX: usize = 8;
pub const IMAGE_TAG_LEN: usize = 16;
pub const STORED_IMAGES_MAX: usize = 40;

pub const TARGET_CABINET: &str = "cabinet";
pub const TARGET_BOT: &str = "bot";
pub const TARGET_SUPPORT: &str = "support";

const NAMED_TARGETS: [&str; 3] = [TARGET_CABINET, TARGET_BOT, TARGET_SUPPORT];
const URL_HINT: &str = "нужна ссылка вида https://…, http://… или t.me/…";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub title: usize,
    pub text: usize,
    pub answer: usize,
    pub note: usize,
    pub plans_text: usize,
    pub label: usize,
    pub menu: usize,
    pub app: usize,
    pub footer_text: usize,
    pub url: usize,
    pub features: usize,
    pub steps: usize,
    pub faq: usize,
    pub apps: usize,
    pub hero_buttons: usize,
    pub cta_buttons: usize,
    pub footer_links: usize,
    pub page_css: usize,
    pub block_css: usize,
}

pub const LIMITS: Limits = Limits {
    title: TITLE_MAX,
    text: TEXT_MAX,
    answer: ANSWER_MAX,
    note: NOTE_MAX,
    plans_text: PLANS_TEXT_MAX,
    label: LABEL_MAX,
    menu: MENU_MAX,
    app: APP_MAX,
    footer_text: FOOTER_TEXT_MAX,
    url: URL_MAX,
    features: FEATURES_MAX,
    steps: STEPS_MAX,
    faq: FAQ_MAX,
    apps: APPS_MAX,
    hero_buttons: HERO_BUTTONS_MAX,
    cta_buttons: CTA_BUTTONS_MAX,
    footer_links: FOOTER_LINKS_MAX,
    page_css: PAGE_CSS_MAX,
    block_css: BLOCK_CSS_MAX,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct LandingError(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    Hero,
    Features,
    Steps,
    Plans,
    Servers,
    Faq,
    Cta,
    Footer,
}

impl BlockKind {
    pub const ALL: [BlockKind; 8] = [
        BlockKind::Hero,
        BlockKind::Features,
        BlockKind::Steps,
        BlockKind::Plans,
        BlockKind::Servers,
        BlockKind::Faq,
        BlockKind::Cta,
        BlockKind::Footer,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            BlockKind::Hero => "hero",
            BlockKind::Features => "features",
            BlockKind::Steps => "steps",
            BlockKind::Plans => "plans",
            BlockKind::Servers => "servers",
            BlockKind::Faq => "faq",
            BlockKind::Cta => "cta",
            BlockKind::Footer => "footer",
        }
    }

    pub fn parse(raw: &str) -> Option<BlockKind> {
        BlockKind::ALL.into_iter().find(|kind| kind.as_str() == raw)
    }

    pub const fn title(self) -> &'static str {
        match self {
            BlockKind::Hero => "Первый экран",
            BlockKind::Features => "Преимущества",
            BlockKind::Steps => "Как подключиться",
            BlockKind::Plans => "Тарифы",
            BlockKind::Servers => "Серверы и приложения",
            BlockKind::Faq => "Вопросы и ответы",
            BlockKind::Cta => "Призыв в конце",
            BlockKind::Footer => "Подвал",
        }
    }

    pub const fn pinned(self) -> bool {
        matches!(self, BlockKind::Hero | BlockKind::Footer)
    }

    pub const fn std_items(self) -> &'static [&'static str] {
        match self {
            BlockKind::Features => &["cabinet", "bot"],
            BlockKind::Steps => &["pick", "pay", "connect"],
            BlockKind::Faq => &["channels", "devices", "renewal", "refund", "privacy"],
            _ => &[],
        }
    }

    pub const fn items_max(self) -> usize {
        match self {
            BlockKind::Features => FEATURES_MAX,
            BlockKind::Steps => STEPS_MAX,
            BlockKind::Faq => FAQ_MAX,
            _ => 0,
        }
    }

    const fn item_word(self) -> &'static str {
        match self {
            BlockKind::Faq => "вопрос",
            _ => "пункт",
        }
    }
}

fn yes() -> bool {
    true
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn or_empty<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    #[serde(default, deserialize_with = "or_empty")]
    pub label: String,
    #[serde(default, deserialize_with = "or_empty")]
    pub to: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LandingImage {
    #[serde(default, deserialize_with = "or_empty")]
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ImageKind>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub width: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub std: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<Link>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<LandingImage>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Art {
    #[default]
    #[serde(rename = "default")]
    Standard,
    #[serde(rename = "image")]
    Image,
    #[serde(rename = "none")]
    Hidden,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hero {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buttons: Option<Vec<Link>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<Art>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<LandingImage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListBlock {
    #[serde(default = "yes")]
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<Item>>,
}

impl Default for ListBlock {
    fn default() -> Self {
        ListBlock {
            on: true,
            title: None,
            menu: None,
            items: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plans {
    #[serde(default = "yes")]
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl Default for Plans {
    fn default() -> Self {
        Plans {
            on: true,
            title: None,
            menu: None,
            text: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Servers {
    #[serde(default = "yes")]
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servers_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servers_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps_note: Option<String>,
}

impl Default for Servers {
    fn default() -> Self {
        Servers {
            on: true,
            menu: None,
            servers_title: None,
            servers_note: None,
            apps_title: None,
            apps: None,
            apps_note: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cta {
    #[serde(default = "yes")]
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buttons: Option<Vec<Link>>,
}

impl Default for Cta {
    fn default() -> Self {
        Cta {
            on: true,
            title: None,
            text: None,
            buttons: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Footer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<Link>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Hero(Hero),
    Features(ListBlock),
    Steps(ListBlock),
    Plans(Plans),
    Servers(Servers),
    Faq(ListBlock),
    Cta(Cta),
    Footer(Footer),
}

impl Block {
    pub fn standard(kind: BlockKind) -> Block {
        match kind {
            BlockKind::Hero => Block::Hero(Hero::default()),
            BlockKind::Features => Block::Features(ListBlock::default()),
            BlockKind::Steps => Block::Steps(ListBlock::default()),
            BlockKind::Plans => Block::Plans(Plans::default()),
            BlockKind::Servers => Block::Servers(Servers::default()),
            BlockKind::Faq => Block::Faq(ListBlock::default()),
            BlockKind::Cta => Block::Cta(Cta::default()),
            BlockKind::Footer => Block::Footer(Footer::default()),
        }
    }

    pub const fn kind(&self) -> BlockKind {
        match self {
            Block::Hero(_) => BlockKind::Hero,
            Block::Features(_) => BlockKind::Features,
            Block::Steps(_) => BlockKind::Steps,
            Block::Plans(_) => BlockKind::Plans,
            Block::Servers(_) => BlockKind::Servers,
            Block::Faq(_) => BlockKind::Faq,
            Block::Cta(_) => BlockKind::Cta,
            Block::Footer(_) => BlockKind::Footer,
        }
    }

    pub const fn on(&self) -> bool {
        match self {
            Block::Hero(_) | Block::Footer(_) => true,
            Block::Features(list) | Block::Steps(list) | Block::Faq(list) => list.on,
            Block::Plans(plans) => plans.on,
            Block::Servers(servers) => servers.on,
            Block::Cta(cta) => cta.on,
        }
    }

    fn clean(self) -> Result<Block, LandingError> {
        let kind = self.kind();
        let at = At::block(kind);
        Ok(match self {
            Block::Hero(hero) => Block::Hero(hero.clean(&at)?),
            Block::Features(list) => Block::Features(list.clean(kind, &at)?),
            Block::Steps(list) => Block::Steps(list.clean(kind, &at)?),
            Block::Plans(plans) => Block::Plans(plans.clean(&at)?),
            Block::Servers(servers) => Block::Servers(servers.clean(&at)?),
            Block::Faq(list) => Block::Faq(list.clean(kind, &at)?),
            Block::Cta(cta) => Block::Cta(cta.clean(&at)?),
            Block::Footer(footer) => Block::Footer(footer.clean(&at)?),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LandingConfig {
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default, skip_serializing_if = "LandingCss::is_empty")]
    pub css: LandingCss,
}

impl Default for LandingConfig {
    fn default() -> Self {
        LandingConfig::standard()
    }
}

impl LandingConfig {
    pub fn standard() -> LandingConfig {
        LandingConfig {
            blocks: BlockKind::ALL.into_iter().map(Block::standard).collect(),
            css: LandingCss::default(),
        }
    }

    pub fn is_standard(&self) -> bool {
        *self == LandingConfig::standard()
    }

    pub fn parse(raw: &[Value]) -> Result<LandingConfig, LandingError> {
        let mut seen = BTreeSet::new();
        let mut blocks = Vec::with_capacity(raw.len());
        for (index, value) in raw.iter().enumerate() {
            let Some(name) = value.get("kind").and_then(Value::as_str) else {
                return Err(LandingError(format!(
                    "Блок {}: не указано, что это за блок",
                    index + 1
                )));
            };
            let Some(kind) = BlockKind::parse(name) else {
                let shown: String = name.chars().take(40).collect();
                return Err(LandingError(format!(
                    "Блок {}: неизвестный блок «{shown}»",
                    index + 1
                )));
            };
            if !seen.insert(kind) {
                return Err(LandingError(format!(
                    "{}: блок указан дважды",
                    kind.title()
                )));
            }
            let block: Block = serde_json::from_value(value.clone()).map_err(|err| {
                LandingError(format!("{}: поля заполнены неверно ({err})", kind.title()))
            })?;
            blocks.push(block.clean()?);
        }
        Ok(LandingConfig {
            blocks,
            css: LandingCss::default(),
        }
        .arranged())
    }

    pub fn arranged(self) -> LandingConfig {
        let mut seen = BTreeSet::new();
        let mut hero = None;
        let mut footer = None;
        let mut middle: Vec<Block> = Vec::new();
        for block in self.blocks {
            let kind = block.kind();
            if !seen.insert(kind) {
                continue;
            }
            match kind {
                BlockKind::Hero => hero = Some(block),
                BlockKind::Footer => footer = Some(block),
                _ => middle.push(block),
            }
        }
        for (place, kind) in BlockKind::ALL.into_iter().enumerate() {
            if kind.pinned() || seen.contains(&kind) {
                continue;
            }
            let at = BlockKind::ALL[..place]
                .iter()
                .rev()
                .find_map(|before| {
                    middle
                        .iter()
                        .position(|block| block.kind() == *before)
                        .map(|index| index + 1)
                })
                .unwrap_or(0);
            middle.insert(at, Block::standard(kind));
            seen.insert(kind);
        }
        let mut blocks = Vec::with_capacity(middle.len() + 2);
        blocks.push(hero.unwrap_or_else(|| Block::standard(BlockKind::Hero)));
        blocks.extend(middle);
        blocks.push(footer.unwrap_or_else(|| Block::standard(BlockKind::Footer)));
        LandingConfig {
            blocks,
            css: self.css,
        }
    }

    pub fn images(&self) -> Vec<&LandingImage> {
        let mut found = Vec::new();
        for block in &self.blocks {
            match block {
                Block::Hero(hero) => found.extend(hero.image.as_ref()),
                Block::Features(list) | Block::Steps(list) | Block::Faq(list) => found.extend(
                    list.items
                        .iter()
                        .flatten()
                        .filter_map(|item| item.image.as_ref()),
                ),
                _ => {}
            }
        }
        found
    }

    pub fn images_mut(&mut self) -> Vec<(String, &mut LandingImage)> {
        let mut found = Vec::new();
        for block in &mut self.blocks {
            let kind = block.kind();
            match block {
                Block::Hero(hero) => {
                    if let Some(image) = hero.image.as_mut() {
                        found.push((format!("{}, картинка", kind.title()), image));
                    }
                }
                Block::Features(list) | Block::Steps(list) | Block::Faq(list) => {
                    for (index, item) in list.items.iter_mut().flatten().enumerate() {
                        if let Some(image) = item.image.as_mut() {
                            found.push((
                                format!(
                                    "{}, {} {}, картинка",
                                    kind.title(),
                                    kind.item_word(),
                                    index + 1
                                ),
                                image,
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        found
    }

    pub fn image_tags(&self) -> BTreeSet<String> {
        self.images()
            .into_iter()
            .map(|image| image.tag.clone())
            .collect()
    }

    pub fn with_urls(&self) -> LandingConfig {
        let mut copy = self.clone();
        for (_, image) in copy.images_mut() {
            image.url = image.kind.map(|kind| image_path(&image.tag, kind));
        }
        copy
    }
}

pub fn image_path(tag: &str, kind: ImageKind) -> String {
    format!("/api/public/landing/{tag}.{}", kind.extension())
}

pub fn valid_tag(tag: &str) -> bool {
    tag.len() == IMAGE_TAG_LEN
        && tag
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn clean_line(raw: &str) -> String {
    let kept: String = raw
        .chars()
        .filter(|c| c.is_whitespace() || !c.is_control())
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn clean_paragraph(raw: &str) -> String {
    let unified = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = Vec::new();
    for line in unified.split('\n') {
        let line = clean_line(line);
        if line.is_empty() && lines.last().is_none_or(String::is_empty) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

pub fn target(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("укажите, куда ведёт".into());
    }
    if NAMED_TARGETS.contains(&raw) {
        return Ok(raw.to_string());
    }
    if let Some(anchor) = raw.strip_prefix('#') {
        return match BlockKind::parse(anchor) {
            Some(kind) if !kind.pinned() => Ok(raw.to_string()),
            _ => Err("такого блока на странице нет".into()),
        };
    }
    let url = match (raw.get(..5), raw.get(5..)) {
        (Some(head), Some(rest)) if head.eq_ignore_ascii_case("t.me/") && !rest.is_empty() => {
            format!("https://t.me/{rest}")
        }
        _ => raw.to_string(),
    };
    web_url(&url)
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn web_url(url: &str) -> Result<String, String> {
    if url.chars().count() > URL_MAX {
        return Err(format!("не длиннее {URL_MAX} символов"));
    }
    let start = if starts_with_ignore_case(url, "https://") {
        "https://".len()
    } else if starts_with_ignore_case(url, "http://") {
        "http://".len()
    } else {
        return Err(URL_HINT.into());
    };
    if url.chars().any(|c| {
        c.is_whitespace()
            || c.is_control()
            || matches!(
                c,
                '"' | '\'' | '<' | '>' | '`' | '\\' | '{' | '}' | '|' | '^'
            )
    }) {
        return Err("в ссылке не должно быть пробелов, кавычек и скобок".into());
    }
    let rest = &url[start..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
        && host.chars().any(char::is_alphanumeric)
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-']);
    let port_ok =
        port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()));
    if !host_ok || !port_ok {
        return Err("в ссылке нет адреса сайта".into());
    }
    if host.eq_ignore_ascii_case("t.me") {
        return Ok(format!("https://t.me{tail}"));
    }
    Ok(url.to_string())
}

struct At(String);

impl At {
    fn block(kind: BlockKind) -> At {
        At(kind.title().to_string())
    }

    fn then(&self, part: impl Display) -> At {
        At(format!("{}, {part}", self.0))
    }

    fn fail(&self, reason: impl Display) -> LandingError {
        LandingError(format!("{}: {reason}", self.0))
    }
}

fn fits(text: &str, at: &At, max: usize) -> Result<(), LandingError> {
    if text.chars().count() > max {
        Err(at.fail(format!("не длиннее {max} символов")))
    } else {
        Ok(())
    }
}

fn required_line(
    value: Option<String>,
    at: &At,
    max: usize,
) -> Result<Option<String>, LandingError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let clean = clean_line(&raw);
    if clean.is_empty() {
        return Err(at.fail("нужно заполнить"));
    }
    fits(&clean, at, max)?;
    Ok(Some(clean))
}

fn optional_line(
    value: Option<String>,
    at: &At,
    max: usize,
) -> Result<Option<String>, LandingError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let clean = clean_line(&raw);
    fits(&clean, at, max)?;
    Ok(Some(clean))
}

fn optional_paragraph(
    value: Option<String>,
    at: &At,
    max: usize,
) -> Result<Option<String>, LandingError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let clean = clean_paragraph(&raw);
    fits(&clean, at, max)?;
    Ok(Some(clean))
}

fn clean_link(
    link: Link,
    at: &At,
    label_name: &str,
    target_name: &str,
) -> Result<Link, LandingError> {
    let label_at = at.then(label_name);
    let label = clean_line(&link.label);
    if label.is_empty() {
        return Err(label_at.fail("нужно заполнить"));
    }
    fits(&label, &label_at, LABEL_MAX)?;
    let to = target(&link.to).map_err(|reason| at.then(target_name).fail(reason))?;
    Ok(Link { label, to })
}

struct LinkWords {
    one: &'static str,
    many: &'static str,
    target: &'static str,
}

const BUTTON_WORDS: LinkWords = LinkWords {
    one: "кнопка",
    many: "кнопки",
    target: "ссылка",
};

const FOOTER_WORDS: LinkWords = LinkWords {
    one: "ссылка",
    many: "ссылки",
    target: "адрес",
};

fn clean_links(
    links: Option<Vec<Link>>,
    at: &At,
    max: usize,
    words: &LinkWords,
) -> Result<Option<Vec<Link>>, LandingError> {
    let Some(links) = links else {
        return Ok(None);
    };
    if links.len() > max {
        return Err(at.then(words.many).fail(format!("не больше {max}")));
    }
    links
        .into_iter()
        .enumerate()
        .map(|(index, link)| {
            clean_link(
                link,
                &at.then(format!("{} {}", words.one, index + 1)),
                "подпись",
                words.target,
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn clean_image(image: LandingImage, at: &At) -> Result<LandingImage, LandingError> {
    let tag = image.tag.trim().to_ascii_lowercase();
    if !valid_tag(&tag) {
        return Err(at.fail("указана неверно, загрузите её заново"));
    }
    Ok(LandingImage {
        tag,
        ..LandingImage::default()
    })
}

fn clean_item(
    kind: BlockKind,
    item: Item,
    at: &At,
    seen: &mut BTreeSet<String>,
) -> Result<Item, LandingError> {
    if let Some(std) = item.std {
        let key = std.trim().to_string();
        if !kind.std_items().contains(&key.as_str()) {
            let shown: String = key.chars().take(40).collect();
            return Err(at.fail(format!("стандартного пункта «{shown}» нет")));
        }
        if item.title.is_some()
            || item.text.is_some()
            || item.link.is_some()
            || item.image.is_some()
        {
            return Err(at.fail("у стандартного пункта не бывает своих полей"));
        }
        if !seen.insert(key.clone()) {
            return Err(at.fail(format!("стандартный пункт «{key}» уже есть в списке")));
        }
        return Ok(Item {
            std: Some(key),
            ..Item::default()
        });
    }
    let faq = kind == BlockKind::Faq;
    let title_at = at.then(if faq {
        "вопрос"
    } else {
        "заголовок"
    });
    let title = clean_line(item.title.as_deref().unwrap_or_default());
    if title.is_empty() {
        return Err(title_at.fail("нужно заполнить"));
    }
    fits(&title, &title_at, TITLE_MAX)?;
    let text_at = at.then(if faq { "ответ" } else { "текст" });
    let text = clean_paragraph(item.text.as_deref().unwrap_or_default());
    if faq && text.is_empty() {
        return Err(text_at.fail("нужно заполнить"));
    }
    fits(&text, &text_at, if faq { ANSWER_MAX } else { TEXT_MAX })?;
    let link = match item.link {
        None => None,
        Some(_) if kind != BlockKind::Features => {
            return Err(at.then("ссылка").fail("в этом блоке у пунктов нет ссылок"));
        }
        Some(link) => Some(clean_link(link, at, "подпись ссылки", "ссылка")?),
    };
    let image = match item.image {
        None => None,
        Some(_) if kind != BlockKind::Features => {
            return Err(at
                .then("картинка")
                .fail("в этом блоке у пунктов нет картинок"));
        }
        Some(image) => Some(clean_image(image, &at.then("картинка"))?),
    };
    Ok(Item {
        std: None,
        title: Some(title),
        text: (!text.is_empty()).then_some(text),
        link,
        image,
    })
}

fn clean_items(
    kind: BlockKind,
    items: Option<Vec<Item>>,
    at: &At,
) -> Result<Option<Vec<Item>>, LandingError> {
    let Some(items) = items else {
        return Ok(None);
    };
    let max = kind.items_max();
    if items.len() > max {
        return Err(at.then("пункты").fail(format!("не больше {max}")));
    }
    let mut seen = BTreeSet::new();
    items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            clean_item(
                kind,
                item,
                &at.then(format!("{} {}", kind.item_word(), index + 1)),
                &mut seen,
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

impl Hero {
    fn clean(self, at: &At) -> Result<Hero, LandingError> {
        let title = required_line(self.title, &at.then("заголовок"), TITLE_MAX)?;
        let lead = optional_paragraph(self.lead, &at.then("текст"), TEXT_MAX)?;
        let note = optional_paragraph(self.note, &at.then("строка под кнопками"), NOTE_MAX)?;
        let buttons = clean_links(self.buttons, at, HERO_BUTTONS_MAX, &BUTTON_WORDS)?;
        let image = self
            .image
            .map(|image| clean_image(image, &at.then("картинка")))
            .transpose()?;
        let art = match self.art {
            None | Some(Art::Standard) => None,
            Some(Art::Image) if image.is_none() => {
                return Err(at
                    .then("картинка")
                    .fail("загрузите картинку или выберите стандартную иллюстрацию"));
            }
            other => other,
        };
        Ok(Hero {
            title,
            lead,
            note,
            buttons,
            art,
            image,
        })
    }
}

impl ListBlock {
    fn clean(self, kind: BlockKind, at: &At) -> Result<ListBlock, LandingError> {
        Ok(ListBlock {
            on: self.on,
            title: required_line(self.title, &at.then("заголовок"), TITLE_MAX)?,
            menu: optional_line(self.menu, &at.then("пункт меню"), MENU_MAX)?,
            items: clean_items(kind, self.items, at)?,
        })
    }
}

impl Plans {
    fn clean(self, at: &At) -> Result<Plans, LandingError> {
        Ok(Plans {
            on: self.on,
            title: required_line(self.title, &at.then("заголовок"), TITLE_MAX)?,
            menu: optional_line(self.menu, &at.then("пункт меню"), MENU_MAX)?,
            text: optional_paragraph(self.text, &at.then("текст"), PLANS_TEXT_MAX)?,
        })
    }
}

impl Servers {
    fn clean(self, at: &At) -> Result<Servers, LandingError> {
        let apps = match self.apps {
            None => None,
            Some(apps) => {
                if apps.len() > APPS_MAX {
                    return Err(at.then("приложения").fail(format!("не больше {APPS_MAX}")));
                }
                let mut clean = Vec::with_capacity(apps.len());
                for (index, name) in apps.iter().enumerate() {
                    let at = at.then(format!("приложение {}", index + 1));
                    let name = clean_line(name);
                    if name.is_empty() {
                        return Err(at.fail("нужно заполнить"));
                    }
                    fits(&name, &at, APP_MAX)?;
                    clean.push(name);
                }
                Some(clean)
            }
        };
        Ok(Servers {
            on: self.on,
            menu: optional_line(self.menu, &at.then("пункт меню"), MENU_MAX)?,
            servers_title: required_line(
                self.servers_title,
                &at.then("заголовок про серверы"),
                TITLE_MAX,
            )?,
            servers_note: optional_paragraph(
                self.servers_note,
                &at.then("текст про серверы"),
                TEXT_MAX,
            )?,
            apps_title: required_line(
                self.apps_title,
                &at.then("заголовок про приложения"),
                TITLE_MAX,
            )?,
            apps,
            apps_note: optional_paragraph(
                self.apps_note,
                &at.then("текст про приложения"),
                TEXT_MAX,
            )?,
        })
    }
}

impl Cta {
    fn clean(self, at: &At) -> Result<Cta, LandingError> {
        Ok(Cta {
            on: self.on,
            title: required_line(self.title, &at.then("заголовок"), TITLE_MAX)?,
            text: optional_paragraph(self.text, &at.then("текст"), TEXT_MAX)?,
            buttons: clean_links(self.buttons, at, CTA_BUTTONS_MAX, &BUTTON_WORDS)?,
        })
    }
}

impl Footer {
    fn clean(self, at: &At) -> Result<Footer, LandingError> {
        Ok(Footer {
            text: optional_line(self.text, &at.then("текст"), FOOTER_TEXT_MAX)?,
            links: clean_links(self.links, at, FOOTER_LINKS_MAX, &FOOTER_WORDS)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Result<LandingConfig, LandingError> {
        LandingConfig::parse(value.as_array().unwrap())
    }

    fn kinds(config: &LandingConfig) -> Vec<&'static str> {
        config.blocks.iter().map(|b| b.kind().as_str()).collect()
    }

    fn message(result: Result<LandingConfig, LandingError>) -> String {
        result.unwrap_err().0
    }

    fn block(config: &LandingConfig, kind: BlockKind) -> &Block {
        config.blocks.iter().find(|b| b.kind() == kind).unwrap()
    }

    #[test]
    fn стандартный_лендинг_восемь_блоков_по_порядку() {
        let standard = LandingConfig::standard();
        assert_eq!(
            kinds(&standard),
            [
                "hero", "features", "steps", "plans", "servers", "faq", "cta", "footer"
            ]
        );
        assert!(standard.blocks.iter().all(Block::on));
        assert_eq!(parse(json!([])).unwrap(), standard);
        assert!(parse(json!([])).unwrap().is_standard());
        assert_eq!(LandingConfig::default(), standard);
        let stored = serde_json::to_value(&standard).unwrap();
        assert_eq!(stored["blocks"][0], json!({ "kind": "hero" }));
        assert_eq!(
            stored["blocks"][1],
            json!({ "kind": "features", "on": true })
        );
        assert_eq!(stored["blocks"][7], json!({ "kind": "footer" }));
        let back: LandingConfig = serde_json::from_value(stored).unwrap();
        assert_eq!(back, standard);
    }

    #[test]
    fn первый_экран_сверху_подвал_снизу_а_недостающие_встают_на_свои_места() {
        let config = parse(json!([
            { "kind": "footer" },
            { "kind": "faq" },
            { "kind": "hero", "on": false },
            { "kind": "plans", "on": false }
        ]))
        .unwrap();
        assert_eq!(
            kinds(&config),
            [
                "hero", "features", "steps", "faq", "cta", "plans", "servers", "footer"
            ]
        );
        assert!(block(&config, BlockKind::Hero).on());
        assert!(!block(&config, BlockKind::Plans).on());
        assert!(block(&config, BlockKind::Steps).on());
        let lenient = LandingConfig {
            blocks: vec![
                Block::standard(BlockKind::Cta),
                Block::standard(BlockKind::Cta),
                Block::standard(BlockKind::Footer),
                Block::standard(BlockKind::Hero),
            ],
            css: LandingCss::default(),
        }
        .arranged();
        assert_eq!(
            kinds(&lenient),
            [
                "hero", "features", "steps", "plans", "servers", "faq", "cta", "footer"
            ]
        );
    }

    #[test]
    fn повтор_и_неизвестный_блок_отклоняются() {
        assert_eq!(
            message(parse(json!([{ "kind": "faq" }, { "kind": "faq" }]))),
            "Вопросы и ответы: блок указан дважды"
        );
        assert_eq!(
            message(parse(json!([{ "kind": "hero" }, { "kind": "video" }]))),
            "Блок 2: неизвестный блок «video»"
        );
        assert_eq!(
            message(parse(json!([{ "title": "Без вида" }]))),
            "Блок 1: не указано, что это за блок"
        );
        assert!(
            message(parse(json!([{ "kind": "hero", "title": 5 }])))
                .starts_with("Первый экран: поля заполнены неверно")
        );
    }

    #[test]
    fn заголовки_и_подписи_проверяются_по_длине_и_пустоте() {
        let long = "Ж".repeat(TITLE_MAX + 1);
        assert_eq!(
            message(parse(json!([{ "kind": "hero", "title": long }]))),
            "Первый экран, заголовок: не длиннее 120 символов"
        );
        assert_eq!(
            message(parse(json!([{ "kind": "hero", "title": "   " }]))),
            "Первый экран, заголовок: нужно заполнить"
        );
        assert_eq!(
            message(parse(json!([{ "kind": "plans", "title": "" }]))),
            "Тарифы, заголовок: нужно заполнить"
        );
        assert_eq!(
            message(parse(json!([{ "kind": "servers", "apps_title": "" }]))),
            "Серверы и приложения, заголовок про приложения: нужно заполнить"
        );
        assert_eq!(
            message(parse(json!([
                { "kind": "hero", "buttons": [{ "label": "", "to": "cabinet" }] }
            ]))),
            "Первый экран, кнопка 1, подпись: нужно заполнить"
        );
        assert_eq!(
            message(parse(json!([
                { "kind": "cta", "buttons": [{ "label": "Ж".repeat(LABEL_MAX + 1), "to": "bot" }] }
            ]))),
            "Призыв в конце, кнопка 1, подпись: не длиннее 40 символов"
        );
        let exact = "Ж".repeat(TITLE_MAX);
        let config = parse(json!([{
            "kind": "hero",
            "title": format!("  {exact}  "),
            "lead": "",
            "note": ""
        }]))
        .unwrap();
        let Block::Hero(hero) = block(&config, BlockKind::Hero) else {
            unreachable!()
        };
        assert_eq!(hero.title.as_deref(), Some(exact.as_str()));
        assert_eq!(hero.lead.as_deref(), Some(""));
        assert_eq!(hero.note.as_deref(), Some(""));
        let stored = serde_json::to_value(&config).unwrap();
        assert_eq!(stored["blocks"][0]["lead"], "");
        let config = parse(json!([
            { "kind": "faq", "menu": "", "title": null },
            { "kind": "footer", "text": "" }
        ]))
        .unwrap();
        let Block::Faq(faq) = block(&config, BlockKind::Faq) else {
            unreachable!()
        };
        assert_eq!(faq.menu.as_deref(), Some(""));
        assert_eq!(faq.title, None);
        let Block::Footer(footer) = block(&config, BlockKind::Footer) else {
            unreachable!()
        };
        assert_eq!(footer.text.as_deref(), Some(""));
    }

    #[test]
    fn ссылки_только_на_сайты_и_telegram() {
        assert_eq!(target("cabinet").unwrap(), "cabinet");
        assert_eq!(target(" bot ").unwrap(), "bot");
        assert_eq!(target("support").unwrap(), "support");
        assert_eq!(target("#plans").unwrap(), "#plans");
        assert_eq!(target("#features").unwrap(), "#features");
        assert!(target("#hero").is_err());
        assert!(target("#footer").is_err());
        assert!(target("#video").is_err());
        assert_eq!(
            target("https://example.com/a?b=1#c").unwrap(),
            "https://example.com/a?b=1#c"
        );
        assert_eq!(target("http://example.com").unwrap(), "http://example.com");
        assert_eq!(
            target("https://пример.рф/оферта").unwrap(),
            "https://пример.рф/оферта"
        );
        assert_eq!(
            target("https://example.com:8443/x").unwrap(),
            "https://example.com:8443/x"
        );
        assert_eq!(
            target("t.me/stealthnet").unwrap(),
            "https://t.me/stealthnet"
        );
        assert_eq!(
            target("T.ME/stealthnet").unwrap(),
            "https://t.me/stealthnet"
        );
        assert_eq!(
            target("http://t.me/support_bot").unwrap(),
            "https://t.me/support_bot"
        );
        assert_eq!(
            target("https://T.me/x?start=1").unwrap(),
            "https://t.me/x?start=1"
        );
        for bad in [
            "",
            "javascript:alert(1)",
            "JavaScript://example.com/%0Aalert(1)",
            "tg://resolve?domain=stealthnet",
            "data:text/html,<b>x</b>",
            "/cabinet",
            "cabinet/pay",
            "example.com",
            "ftp://example.com",
            "https://",
            "https:///path",
            "https://exa mple.com",
            "https://example.com/\"onmouseover",
            "https://example.com/<script>",
            "https://user@evil.com",
            "https://example.com:port/x",
            "https://.example.com",
            "t.me/",
            "пример.рф",
            "ыt.me/x",
        ] {
            assert!(target(bad).is_err(), "{bad}");
        }
        let long = format!("https://example.com/{}", "a".repeat(URL_MAX));
        assert_eq!(target(&long).unwrap_err(), "не длиннее 500 символов");
        assert_eq!(
            message(parse(json!([
                { "kind": "hero", "buttons": [{ "label": "Открыть", "to": "javascript:alert(1)" }] }
            ]))),
            "Первый экран, кнопка 1, ссылка: нужна ссылка вида https://…, http://… или t.me/…"
        );
        assert_eq!(
            message(parse(json!([
                { "kind": "footer", "links": [{ "label": "Канал", "to": "tg://resolve" }] }
            ]))),
            "Подвал, ссылка 1, адрес: нужна ссылка вида https://…, http://… или t.me/…"
        );
        let config = parse(json!([{
            "kind": "features",
            "items": [{ "title": "Канал", "link": { "label": "Подписаться", "to": "t.me/news" } }]
        }]))
        .unwrap();
        let Block::Features(list) = block(&config, BlockKind::Features) else {
            unreachable!()
        };
        let link = list.items.as_ref().unwrap()[0].link.as_ref().unwrap();
        assert_eq!(link.to, "https://t.me/news");
    }

    #[test]
    fn стандартные_пункты_только_свои_и_без_своих_полей() {
        let config = parse(json!([
            { "kind": "features", "items": [{ "std": "bot" }, { "std": "cabinet" }] },
            { "kind": "steps", "items": [{ "std": "pay" }, { "title": "Своё", "text": "" }] },
            { "kind": "faq", "items": [{ "std": "devices", "title": null }] }
        ]))
        .unwrap();
        let Block::Steps(steps) = block(&config, BlockKind::Steps) else {
            unreachable!()
        };
        let items = steps.items.as_ref().unwrap();
        assert_eq!(items[0].std.as_deref(), Some("pay"));
        assert_eq!(items[1].title.as_deref(), Some("Своё"));
        assert_eq!(items[1].text, None);
        assert_eq!(
            serde_json::to_value(items).unwrap(),
            json!([{ "std": "pay" }, { "title": "Своё" }])
        );
        assert_eq!(
            message(parse(
                json!([{ "kind": "features", "items": [{ "std": "pay" }] }])
            )),
            "Преимущества, пункт 1: стандартного пункта «pay» нет"
        );
        assert_eq!(
            message(parse(json!([
                { "kind": "faq", "items": [{ "std": "refund", "text": "Нет" }] }
            ]))),
            "Вопросы и ответы, вопрос 1: у стандартного пункта не бывает своих полей"
        );
        assert_eq!(
            message(parse(json!([
                { "kind": "steps", "items": [{ "std": "pay" }, { "std": "pay" }] }
            ]))),
            "Как подключиться, пункт 2: стандартный пункт «pay» уже есть в списке"
        );
    }

    #[test]
    fn у_пунктов_проверяются_заголовок_ответ_ссылка_и_картинка() {
        assert_eq!(
            message(parse(json!([
                { "kind": "faq", "items": [{ "title": "Есть пробный период?" }] }
            ]))),
            "Вопросы и ответы, вопрос 1, ответ: нужно заполнить"
        );
        assert_eq!(
            message(parse(
                json!([{ "kind": "steps", "items": [{ "text": "Без заголовка" }] }])
            )),
            "Как подключиться, пункт 1, заголовок: нужно заполнить"
        );
        assert_eq!(
            message(parse(json!([{
                "kind": "steps",
                "items": [{ "title": "Шаг", "link": { "label": "Тут", "to": "bot" } }]
            }]))),
            "Как подключиться, пункт 1, ссылка: в этом блоке у пунктов нет ссылок"
        );
        assert_eq!(
            message(parse(json!([{
                "kind": "faq",
                "items": [{ "title": "Вопрос", "text": "Ответ", "image": { "tag": "0123456789abcdef" } }]
            }]))),
            "Вопросы и ответы, вопрос 1, картинка: в этом блоке у пунктов нет картинок"
        );
        assert_eq!(
            message(parse(json!([{
                "kind": "features",
                "items": [{ "title": "Кабинет", "image": { "tag": "../../etc/passwd" } }]
            }]))),
            "Преимущества, пункт 1, картинка: указана неверно, загрузите её заново"
        );
        assert_eq!(
            message(parse(json!([{
                "kind": "features",
                "items": [{ "title": "Кабинет", "link": { "label": "Открыть", "to": "" } }]
            }]))),
            "Преимущества, пункт 1, ссылка: укажите, куда ведёт"
        );
        let long_answer = "ж".repeat(ANSWER_MAX);
        assert!(
            parse(json!([{
                "kind": "faq",
                "items": [{ "title": "Вопрос", "text": long_answer }]
            }]))
            .is_ok()
        );
        let long_text = "ж".repeat(TEXT_MAX + 1);
        assert_eq!(
            message(parse(json!([{
                "kind": "features",
                "items": [{ "title": "Кабинет", "text": long_text }]
            }]))),
            "Преимущества, пункт 1, текст: не длиннее 600 символов"
        );
    }

    #[test]
    fn число_пунктов_кнопок_и_приложений_ограничено() {
        let buttons: Vec<Value> = (0..3)
            .map(|_| json!({ "label": "Купить", "to": "cabinet" }))
            .collect();
        assert_eq!(
            message(parse(json!([{ "kind": "hero", "buttons": buttons }]))),
            "Первый экран, кнопки: не больше 2"
        );
        let questions: Vec<Value> = (0..=FAQ_MAX)
            .map(|i| json!({ "title": format!("Вопрос {i}"), "text": "Ответ" }))
            .collect();
        assert_eq!(
            message(parse(json!([{ "kind": "faq", "items": questions }]))),
            "Вопросы и ответы, пункты: не больше 20"
        );
        let apps: Vec<String> = (0..=APPS_MAX).map(|i| format!("App {i}")).collect();
        assert_eq!(
            message(parse(json!([{ "kind": "servers", "apps": apps }]))),
            "Серверы и приложения, приложения: не больше 12"
        );
        assert_eq!(
            message(parse(json!([{ "kind": "servers", "apps": ["Happ", " "] }]))),
            "Серверы и приложения, приложение 2: нужно заполнить"
        );
        let links: Vec<Value> = (0..=FOOTER_LINKS_MAX)
            .map(|i| json!({ "label": format!("Ссылка {i}"), "to": "https://example.com" }))
            .collect();
        assert_eq!(
            message(parse(json!([{ "kind": "footer", "links": links }]))),
            "Подвал, ссылки: не больше 8"
        );
        let config = parse(json!([
            { "kind": "faq", "items": [] },
            { "kind": "servers", "apps": [] },
            { "kind": "hero", "buttons": [] }
        ]))
        .unwrap();
        let Block::Faq(faq) = block(&config, BlockKind::Faq) else {
            unreachable!()
        };
        assert_eq!(faq.items, Some(vec![]));
        let Block::Hero(hero) = block(&config, BlockKind::Hero) else {
            unreachable!()
        };
        assert_eq!(hero.buttons, Some(vec![]));
    }

    #[test]
    fn абзацы_сохраняют_переносы_строк_а_строки_нет() {
        assert_eq!(
            clean_paragraph("  Первая   строка \r\n\r\n\r\n вторая\u{0}\rтретья\n\n "),
            "Первая строка\n\nвторая\nтретья"
        );
        assert_eq!(clean_paragraph("\n\n"), "");
        assert_eq!(clean_line(" a\n b\t\u{7}c "), "a b c");
        let config = parse(json!([{
            "kind": "hero",
            "title": "Подключение\nза минуту",
            "lead": "Первая\r\nвторая"
        }]))
        .unwrap();
        let Block::Hero(hero) = block(&config, BlockKind::Hero) else {
            unreachable!()
        };
        assert_eq!(hero.title.as_deref(), Some("Подключение за минуту"));
        assert_eq!(hero.lead.as_deref(), Some("Первая\nвторая"));
    }

    #[test]
    fn своя_картинка_первого_экрана_нужна_при_выборе_картинки() {
        assert_eq!(
            message(parse(json!([{ "kind": "hero", "art": "image" }]))),
            "Первый экран, картинка: загрузите картинку или выберите стандартную иллюстрацию"
        );
        let config = parse(json!([{
            "kind": "hero",
            "art": "image",
            "image": { "tag": "0123456789ABCDEF", "kind": "png", "width": 9, "height": 9, "url": "https://evil.example/x.png" }
        }]))
        .unwrap();
        let Block::Hero(hero) = block(&config, BlockKind::Hero) else {
            unreachable!()
        };
        assert_eq!(hero.art, Some(Art::Image));
        assert_eq!(
            hero.image,
            Some(LandingImage {
                tag: "0123456789abcdef".into(),
                ..LandingImage::default()
            })
        );
        let config = parse(json!([{ "kind": "hero", "art": "default" }])).unwrap();
        assert!(config.is_standard());
        let config = parse(json!([{ "kind": "hero", "art": "none" }])).unwrap();
        let Block::Hero(hero) = block(&config, BlockKind::Hero) else {
            unreachable!()
        };
        assert_eq!(hero.art, Some(Art::Hidden));
        assert_eq!(
            serde_json::to_value(hero).unwrap(),
            json!({ "art": "none" })
        );
    }

    #[test]
    fn картинки_собираются_со_всех_блоков_и_получают_адрес() {
        let mut config = parse(json!([
            { "kind": "hero", "art": "image", "image": { "tag": "aaaaaaaaaaaaaaaa" } },
            { "kind": "features", "items": [
                { "std": "cabinet" },
                { "title": "Бот", "image": { "tag": "bbbbbbbbbbbbbbbb" } }
            ] }
        ]))
        .unwrap();
        assert_eq!(
            config.image_tags().into_iter().collect::<Vec<_>>(),
            ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"]
        );
        let places: Vec<String> = config
            .images_mut()
            .into_iter()
            .map(|(place, image)| {
                image.kind = Some(ImageKind::Webp);
                image.width = 1600;
                image.height = 900;
                place
            })
            .collect();
        assert_eq!(
            places,
            ["Первый экран, картинка", "Преимущества, пункт 2, картинка"]
        );
        let shown = config.with_urls();
        let Block::Hero(hero) = block(&shown, BlockKind::Hero) else {
            unreachable!()
        };
        assert_eq!(
            hero.image.as_ref().unwrap().url.as_deref(),
            Some("/api/public/landing/aaaaaaaaaaaaaaaa.webp")
        );
        assert!(config.images().iter().all(|image| image.url.is_none()));
        assert_eq!(
            serde_json::to_value(&shown).unwrap()["blocks"][0]["image"],
            json!({
                "tag": "aaaaaaaaaaaaaaaa",
                "kind": "webp",
                "width": 1600,
                "height": 900,
                "url": "/api/public/landing/aaaaaaaaaaaaaaaa.webp"
            })
        );
        assert!(valid_tag("0123456789abcdef"));
        assert!(!valid_tag("0123456789ABCDEF"));
        assert!(!valid_tag("0123456789abcde"));
        assert!(!valid_tag("0123456789abcdeg"));
    }
}
