use crate::landing::valid_tag;
use cssparser::{ParseError, ParseErrorKind, Parser, Token};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

pub const PAGE_CSS_MAX: usize = 50_000;
pub const BLOCK_CSS_MAX: usize = 10_000;
const DEPTH_MAX: usize = 24;
const SHOWN_MAX: usize = 60;

const GROUPS: [&str; 5] = ["media", "supports", "container", "layer", "starting-style"];
const FRAMES: [&str; 4] = [
    "keyframes",
    "-webkit-keyframes",
    "-moz-keyframes",
    "-o-keyframes",
];
const NAMED: [&str; 5] = [
    "font-face",
    "property",
    "counter-style",
    "position-try",
    "font-palette-values",
];
const PAGE_WIDE: [&str; 4] = ["scope", "page", "view-transition", "-ms-viewport"];
const FORBIDDEN_PROPERTIES: [&str; 4] = [
    "behavior",
    "-ms-behavior",
    "-moz-binding",
    "-webkit-binding",
];
const SCRIPT_SCHEMES: [&str; 2] = ["javascript", "vbscript"];
const HTML_TAGS: [&str; 10] = [
    "style", "script", "link", "html", "head", "body", "svg", "img", "iframe", "div",
];
const FILE_TYPES: [&str; 12] = [
    "png", "jpg", "jpeg", "webp", "gif", "avif", "svg", "ico", "woff", "woff2", "ttf", "otf",
];
const DATA_FONTS: [&str; 6] = [
    "application/font-woff",
    "application/font-woff2",
    "application/x-font-woff",
    "application/x-font-woff2",
    "application/x-font-ttf",
    "application/x-font-otf",
];
const OWN_PAGES: [&str; 9] = [
    "/admin", "/api", "/app", "/cabinet", "/m", "/pay", "/sub", "/verify", "/assets",
];

const FOREIGN_QUOTES: [char; 7] = ['“', '”', '„', '‘', '’', '«', '»'];
const PAGE_ATTRIBUTES: [&str; 3] = ["data-theme", "data-design", "data-surface"];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct CssError {
    pub line: Option<u32>,
    pub reason: String,
}

impl fmt::Display for CssError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "строка {line}: {}", self.reason),
            None => f.write_str(&self.reason),
        }
    }
}

impl CssError {
    fn at(line: u32, reason: impl Into<String>) -> CssError {
        CssError {
            line: Some(line),
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CssSite {
    pub files: &'static str,
    pub classes: &'static [&'static str],
}

pub trait Place: Copy + Ord + fmt::Debug + Serialize + 'static {
    const SITE: CssSite;
    const ALL: &'static [Self];

    fn as_str(self) -> &'static str;

    fn scope(self) -> Option<String>;

    fn title(self) -> String;

    fn parse(raw: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|place| place.as_str() == raw)
    }

    fn limit(self) -> usize {
        if self.scope().is_some() {
            BLOCK_CSS_MAX
        } else {
            PAGE_CSS_MAX
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Space,
    Comment,
    Ident(String),
    At(String),
    Text(String),
    Url(String),
    Function(String),
    Open(char),
    Close(char),
    Colon,
    Semicolon,
    Comma,
    Delim(char),
    Other,
}

impl Kind {
    fn blank(&self) -> bool {
        matches!(self, Kind::Space | Kind::Comment)
    }

    fn opens(&self) -> bool {
        matches!(self, Kind::Function(_) | Kind::Open(_))
    }

    fn closes(&self) -> bool {
        matches!(self, Kind::Close(_))
    }
}

#[derive(Debug, Clone)]
struct Piece {
    kind: Kind,
    start: usize,
    end: usize,
    line: u32,
}

#[derive(Debug, Clone, Copy)]
struct Opened {
    bracket: char,
    line: u32,
}

impl Opened {
    fn closing(self) -> char {
        match self.bracket {
            '(' => ')',
            '[' => ']',
            _ => '}',
        }
    }

    fn unclosed(self) -> CssError {
        CssError::at(self.line, format!("не закрыта скобка {}", self.bracket))
    }
}

struct Reader<'i> {
    source: &'i str,
    pieces: Vec<Piece>,
}

impl<'i> Reader<'i> {
    fn read(
        &mut self,
        parser: &mut Parser<'i>,
        opened: Option<Opened>,
        depth: usize,
    ) -> Result<(), CssError> {
        loop {
            let line = parser.current_source_location().line + 1;
            let start = parser.position().byte_index();
            let Ok(token) = parser.next_including_whitespace_and_comments().cloned() else {
                return Ok(());
            };
            let end = parser.position().byte_index();
            let raw = &self.source[start..end];
            let (kind, bracket) = match token {
                Token::WhiteSpace(_) => (Kind::Space, None),
                Token::Comment(_) => {
                    if raw.len() < 4 || !raw.ends_with("*/") {
                        return Err(CssError::at(line, "не закрыт комментарий /*"));
                    }
                    (Kind::Comment, None)
                }
                Token::Ident(name) => (Kind::Ident(name.to_string()), None),
                Token::AtKeyword(name) => (Kind::At(name.to_string()), None),
                Token::QuotedString(text) => {
                    if raw.len() < 2 || !raw.ends_with(['"', '\'']) {
                        return Err(CssError::at(line, "не закрыта кавычка"));
                    }
                    (Kind::Text(text.to_string()), None)
                }
                Token::UnquotedUrl(url) => {
                    if !raw.ends_with(')') {
                        return Err(CssError::at(line, "не закрыта скобка ("));
                    }
                    (Kind::Url(url.to_string()), None)
                }
                Token::BadString(_) => return Err(CssError::at(line, "не закрыта кавычка")),
                Token::BadUrl(_) => {
                    return Err(CssError::at(
                        line,
                        "адрес в url() написан с ошибкой: возьмите его в кавычки",
                    ));
                }
                Token::Function(name) => (Kind::Function(name.to_string()), Some('(')),
                Token::ParenthesisBlock => (Kind::Open('('), Some('(')),
                Token::SquareBracketBlock => (Kind::Open('['), Some('[')),
                Token::CurlyBracketBlock => (Kind::Open('{'), Some('{')),
                Token::CloseParenthesis => return Err(stray(')', line, opened)),
                Token::CloseSquareBracket => return Err(stray(']', line, opened)),
                Token::CloseCurlyBracket => return Err(stray('}', line, opened)),
                Token::CDO | Token::CDC => {
                    return Err(CssError::at(line, "уберите <!-- и -->: это разметка HTML"));
                }
                Token::Colon => (Kind::Colon, None),
                Token::Semicolon => (Kind::Semicolon, None),
                Token::Comma => (Kind::Comma, None),
                Token::Delim(c) => (Kind::Delim(c), None),
                Token::Hash(_)
                | Token::IDHash(_)
                | Token::Number { .. }
                | Token::Percentage { .. }
                | Token::Dimension { .. }
                | Token::IncludeMatch
                | Token::DashMatch
                | Token::PrefixMatch
                | Token::SuffixMatch
                | Token::SubstringMatch => (Kind::Other, None),
            };
            self.pieces.push(Piece {
                kind,
                start,
                end,
                line,
            });
            if let Some(bracket) = bracket {
                self.nested(parser, Opened { bracket, line }, depth)?;
            }
        }
    }

    fn nested(
        &mut self,
        parser: &mut Parser<'i>,
        opened: Opened,
        depth: usize,
    ) -> Result<(), CssError> {
        let too_deep = || CssError::at(opened.line, "слишком много вложенных скобок");
        if depth >= DEPTH_MAX {
            return Err(too_deep());
        }
        let mut inner = 0;
        parser
            .parse_nested_block(|parser| {
                self.read(parser, Some(opened), depth + 1)
                    .map_err(ParseError::custom)?;
                inner = parser.position().byte_index();
                Ok::<(), ParseError<CssError>>(())
            })
            .map_err(|err| match err.kind {
                ParseErrorKind::Custom(err) => err,
                ParseErrorKind::Basic(_) => too_deep(),
            })?;
        let end = parser.position().byte_index();
        if end == inner {
            return Err(opened.unclosed());
        }
        self.pieces.push(Piece {
            kind: Kind::Close(opened.closing()),
            start: inner,
            end,
            line: parser.current_source_location().line + 1,
        });
        Ok(())
    }
}

fn stray(bracket: char, line: u32, opened: Option<Opened>) -> CssError {
    match opened {
        Some(opened) if opened.bracket != '{' => opened.unclosed(),
        _ => CssError::at(line, format!("лишняя скобка {bracket}")),
    }
}

fn read(source: &str) -> Result<Vec<Piece>, CssError> {
    let mut parser = Parser::new(source);
    let mut reader = Reader {
        source,
        pieces: Vec::new(),
    };
    reader.read(&mut parser, None, 0)?;
    Ok(reader.pieces)
}

fn shown(text: &str) -> String {
    let line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = line.trim();
    if line.chars().count() <= SHOWN_MAX {
        return line.to_string();
    }
    let cut: String = line.chars().take(SHOWN_MAX).collect();
    format!("{cut}…")
}

fn runs_script(text: &str) -> bool {
    let squeezed: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .flat_map(char::to_lowercase)
        .collect();
    SCRIPT_SCHEMES
        .iter()
        .any(|scheme| squeezed.contains(&format!("{scheme}:")))
}

fn script_error(line: u32) -> CssError {
    CssError::at(line, "ссылки javascript: и vbscript: в CSS запрещены")
}

fn own_image(path: &str, site: &CssSite) -> Option<String> {
    let file = path.strip_prefix(site.files)?;
    let (tag, extension) = file.split_once('.')?;
    (valid_tag(tag) && matches!(extension, "png" | "jpg" | "webp")).then(|| tag.to_string())
}

fn has_scheme(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, _)| {
        scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

fn check_url(raw: &str, line: u32, site: &CssSite) -> Result<Option<String>, CssError> {
    if runs_script(raw) {
        return Err(script_error(line));
    }
    let url = raw.trim();
    if url.is_empty() {
        return Err(CssError::at(line, "в url() нет адреса"));
    }
    if url.starts_with('#') {
        return Ok(None);
    }
    if url
        .get(..5)
        .is_some_and(|head| head.eq_ignore_ascii_case("data:"))
    {
        let mime = url[5..]
            .split([',', ';'])
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if mime.starts_with("image/")
            || mime.starts_with("font/")
            || DATA_FONTS.contains(&mime.as_str())
        {
            return Ok(None);
        }
        return Err(CssError::at(
            line,
            "в data: можно вставить только картинку или шрифт",
        ));
    }
    if url.starts_with("//") || has_scheme(url) {
        return Err(CssError::at(
            line,
            format!(
                "адрес «{}» нельзя: картинки и шрифты — только со своего домена. \
                 Загрузите картинку кнопкой под полем",
                shown(url)
            ),
        ));
    }
    if let Some(tag) = own_image(url, site) {
        return Ok(Some(tag));
    }
    let plain = url.starts_with('/')
        && url
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'));
    let file = url.rsplit('/').next().unwrap_or_default();
    let known_file = file
        .rsplit_once('.')
        .is_some_and(|(name, kind)| !name.is_empty() && FILE_TYPES.contains(&kind));
    let own_page = OWN_PAGES
        .iter()
        .any(|page| url == *page || url.starts_with(&format!("{page}/")));
    if !plain || !known_file || own_page || url.contains("//") || url.contains("..") {
        return Err(CssError::at(
            line,
            format!(
                "адрес «{}» нельзя: нужна картинка или шрифт с этого сайта, \
                 например {}0123456789abcdef.png",
                shown(url),
                site.files
            ),
        ));
    }
    Ok(None)
}

fn check_at_rule(name: &str, line: u32) -> Result<(), CssError> {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "import" => Err(CssError::at(
            line,
            "@import запрещён: скопируйте правила из того файла сюда",
        )),
        "charset" => Err(CssError::at(line, "@charset не нужен, уберите эту строку")),
        known
            if GROUPS.contains(&known)
                || FRAMES.contains(&known)
                || NAMED.contains(&known)
                || PAGE_WIDE.contains(&known) =>
        {
            Ok(())
        }
        _ => Err(CssError::at(
            line,
            format!("правило @{} здесь нельзя", shown(&lower)),
        )),
    }
}

fn check_function(name: &str, outer: Option<&str>, line: u32) -> Result<(), CssError> {
    match name {
        "expression" => return Err(CssError::at(line, "expression() запрещён")),
        "src" | "image" => {
            return Err(CssError::at(
                line,
                format!("{name}() здесь нельзя, напишите url()"),
            ));
        }
        _ => {}
    }
    if outer.is_some_and(is_image_set)
        && !matches!(name, "url" | "type")
        && !name.ends_with("gradient")
    {
        return Err(CssError::at(
            line,
            "внутри image-set() адреса пишутся только через url()",
        ));
    }
    Ok(())
}

fn is_image_set(name: &str) -> bool {
    matches!(name, "image-set" | "-webkit-image-set")
}

fn before(pieces: &[Piece], at: usize) -> Option<&Kind> {
    pieces[..at]
        .iter()
        .rev()
        .map(|piece| &piece.kind)
        .find(|kind| !kind.blank())
}

fn after(pieces: &[Piece], at: usize) -> Option<&Kind> {
    pieces[at + 1..]
        .iter()
        .map(|piece| &piece.kind)
        .find(|kind| !kind.blank())
}

fn starts_declaration(pieces: &[Piece], at: usize) -> bool {
    matches!(
        before(pieces, at),
        None | Some(Kind::Semicolon | Kind::Open('{') | Kind::Close('}'))
    ) && matches!(after(pieces, at), Some(Kind::Colon))
}

type Images = BTreeMap<String, u32>;

fn check_pieces(pieces: &[Piece], site: &CssSite) -> Result<Images, CssError> {
    let mut images = Images::new();
    let mut found = |tag: Option<String>, line: u32| {
        if let Some(tag) = tag {
            images.entry(tag).or_insert(line);
        }
    };
    let mut open: Vec<Option<String>> = Vec::new();
    for (at, piece) in pieces.iter().enumerate() {
        let line = piece.line;
        let outer = open.last().and_then(Option::as_deref);
        match &piece.kind {
            Kind::At(name) => check_at_rule(name, line)?,
            Kind::Function(name) => {
                let name = name.to_ascii_lowercase();
                check_function(&name, outer, line)?;
                if name == "url" {
                    found(check_url_call(pieces, at, site)?, line);
                }
            }
            Kind::Url(url) => found(check_url(url, line, site)?, line),
            Kind::Text(text) => {
                if runs_script(text) {
                    return Err(script_error(line));
                }
                if outer.is_some_and(is_image_set) {
                    return Err(CssError::at(
                        line,
                        "внутри image-set() адреса пишутся только через url()",
                    ));
                }
            }
            Kind::Ident(name) => check_ident(pieces, at, name)?,
            Kind::Delim('<') => check_html(pieces, at)?,
            Kind::Delim('/') => check_comment(pieces, at)?,
            Kind::Delim('\\') => {
                return Err(CssError::at(line, "лишний символ \\"));
            }
            _ => {}
        }
        match &piece.kind {
            Kind::Function(name) => open.push(Some(name.to_ascii_lowercase())),
            Kind::Open(_) => open.push(None),
            Kind::Close(_) => {
                open.pop();
            }
            _ => {}
        }
    }
    Ok(images)
}

fn check_url_call(pieces: &[Piece], at: usize, site: &CssSite) -> Result<Option<String>, CssError> {
    let line = pieces[at].line;
    let mut inside = pieces[at + 1..]
        .iter()
        .take_while(|piece| !piece.kind.closes())
        .filter(|piece| !piece.kind.blank());
    match (inside.next().map(|piece| &piece.kind), inside.next()) {
        (Some(Kind::Text(url)), None) => check_url(url, line, site),
        _ => Err(CssError::at(
            line,
            "внутри url() должен быть один адрес в кавычках",
        )),
    }
}

fn check_ident(pieces: &[Piece], at: usize, name: &str) -> Result<(), CssError> {
    let line = pieces[at].line;
    let lower = name.to_ascii_lowercase();
    if SCRIPT_SCHEMES.contains(&lower.as_str())
        && matches!(
            pieces.get(at + 1).map(|piece| &piece.kind),
            Some(Kind::Colon)
        )
    {
        return Err(script_error(line));
    }
    if name.contains(FOREIGN_QUOTES) {
        return Err(CssError::at(
            line,
            "замените кавычки на прямые: \"текст\" или 'текст'",
        ));
    }
    if FORBIDDEN_PROPERTIES.contains(&lower.as_str()) && starts_declaration(pieces, at) {
        return Err(CssError::at(line, format!("свойство {lower} запрещено")));
    }
    Ok(())
}

fn check_comment(pieces: &[Piece], at: usize) -> Result<(), CssError> {
    match pieces.get(at + 1).map(|piece| &piece.kind) {
        Some(Kind::Delim('/')) => Err(CssError::at(
            pieces[at].line,
            "комментарий пишется так: /* текст */",
        )),
        _ => Ok(()),
    }
}

fn check_html(pieces: &[Piece], at: usize) -> Result<(), CssError> {
    let tag = match pieces.get(at + 1).map(|piece| &piece.kind) {
        Some(Kind::Delim('/' | '!')) => true,
        Some(Kind::Ident(name)) => HTML_TAGS.contains(&name.to_ascii_lowercase().as_str()),
        _ => false,
    };
    if tag {
        return Err(CssError::at(
            pieces[at].line,
            "уберите теги вроде <style>: нужен только CSS",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Rule { body: usize },
    Statement,
}

struct Walk<'a> {
    pieces: &'a [Piece],
    at: usize,
    end: usize,
}

impl<'a> Walk<'a> {
    fn new(pieces: &'a [Piece], at: usize, end: usize) -> Walk<'a> {
        Walk { pieces, at, end }
    }

    fn skip_blank(&mut self) {
        while self.at < self.end && self.pieces[self.at].kind.blank() {
            self.at += 1;
        }
    }

    fn variable(&self, at: usize) -> bool {
        matches!(
            self.pieces.get(at).map(|piece| &piece.kind),
            Some(Kind::Ident(name)) if name.starts_with("--")
        ) && matches!(after(self.pieces, at), Some(Kind::Colon))
    }

    fn next(&mut self) -> Option<(usize, usize, Item)> {
        self.skip_blank();
        let start = self.at;
        let variable = self.variable(start);
        let mut depth = 0usize;
        while self.at < self.end {
            let kind = &self.pieces[self.at].kind;
            self.at += 1;
            match kind {
                Kind::Open('{') if depth == 0 && !variable => {
                    let body = self.at - 1;
                    self.at = closing(self.pieces, body) + 1;
                    return Some((start, self.at, Item::Rule { body }));
                }
                Kind::Semicolon if depth == 0 => {
                    return Some((start, self.at, Item::Statement));
                }
                kind if kind.opens() => depth += 1,
                kind if kind.closes() => depth -= 1,
                _ => {}
            }
        }
        (start < self.end).then_some((start, self.end, Item::Statement))
    }
}

fn closing(pieces: &[Piece], open: usize) -> usize {
    let mut depth = 0usize;
    for (at, piece) in pieces.iter().enumerate().skip(open) {
        if piece.kind.opens() {
            depth += 1;
        } else if piece.kind.closes() {
            depth -= 1;
            if depth == 0 {
                return at;
            }
        }
    }
    pieces.len() - 1
}

fn text_of<'a>(source: &'a str, pieces: &[Piece], from: usize, to: usize) -> &'a str {
    match (
        pieces.get(from),
        to.checked_sub(1).and_then(|at| pieces.get(at)),
    ) {
        (Some(first), Some(last)) if from < to => &source[first.start..last.end],
        _ => "",
    }
}

#[derive(Clone, Copy)]
struct Rules<'a> {
    source: &'a str,
    pieces: &'a [Piece],
}

impl<'a> Rules<'a> {
    fn text(&self, from: usize, to: usize) -> &'a str {
        text_of(self.source, self.pieces, from, to)
    }

    fn at_name(&self, at: usize) -> Option<String> {
        match &self.pieces[at].kind {
            Kind::At(name) => Some(name.to_ascii_lowercase()),
            _ => None,
        }
    }

    fn sheet(&self, from: usize, to: usize) -> Result<(), CssError> {
        let mut walk = Walk::new(self.pieces, from, to);
        while let Some((start, end, item)) = walk.next() {
            let line = self.pieces[start].line;
            match (item, self.at_name(start)) {
                (Item::Statement, Some(name)) if name == "layer" => {}
                (Item::Statement, Some(name)) => return Err(no_block(&name, line)),
                (Item::Statement, None) if matches!(self.pieces[start].kind, Kind::Semicolon) => {
                    return Err(CssError::at(line, "лишняя точка с запятой"));
                }
                (Item::Statement, None) => {
                    return Err(CssError::at(
                        line,
                        format!(
                            "«{}» стоит вне правила: нужен селектор и скобки {{ }}",
                            shown(self.text(start, end))
                        ),
                    ));
                }
                (Item::Rule { body }, Some(name)) if GROUPS.contains(&name.as_str()) => {
                    self.sheet(body + 1, end - 1)?;
                }
                (Item::Rule { body }, name) => self.rule(start, body, end, name)?,
            }
        }
        Ok(())
    }

    fn rule(
        &self,
        start: usize,
        body: usize,
        end: usize,
        at_rule: Option<String>,
    ) -> Result<(), CssError> {
        match at_rule {
            None => {
                self.prelude(start, body)?;
                self.body(body + 1, end - 1)
            }
            Some(name) if FRAMES.contains(&name.as_str()) => self.sheet(body + 1, end - 1),
            Some(_) => self.body(body + 1, end - 1),
        }
    }

    fn prelude(&self, start: usize, body: usize) -> Result<(), CssError> {
        if start == body {
            return Err(no_selector(self.pieces[start].line));
        }
        let mut depth = 0usize;
        for at in start..body {
            let kind = &self.pieces[at].kind;
            if kind.opens() {
                depth += 1;
            } else if kind.closes() {
                depth -= 1;
            } else if depth == 0 && matches!(kind, Kind::Colon) {
                let next = after(self.pieces, at);
                if matches!(next, Some(Kind::Open('{'))) {
                    return Err(CssError::at(
                        self.pieces[at].line,
                        "лишнее двоеточие перед скобкой {",
                    ));
                }
                if matches!(self.pieces[at + 1].kind, Kind::Space) {
                    return Err(self.no_semicolon(at, body));
                }
            }
        }
        Ok(())
    }

    fn no_semicolon(&self, colon: usize, to: usize) -> CssError {
        let broken = (colon + 1..to).find(|at| {
            matches!(self.pieces[*at].kind, Kind::Space)
                && self.source[self.pieces[*at].start..self.pieces[*at].end].contains('\n')
                && before(self.pieces, *at) != Some(&Kind::Colon)
        });
        let line = self.pieces[broken.unwrap_or(colon)].line;
        CssError::at(line, "похоже, пропущена точка с запятой")
    }

    fn body(&self, from: usize, to: usize) -> Result<(), CssError> {
        let mut walk = Walk::new(self.pieces, from, to);
        while let Some((start, end, item)) = walk.next() {
            match (item, self.at_name(start)) {
                (Item::Statement, None) => self.declaration(start, end)?,
                (Item::Statement, Some(name)) => {
                    return Err(no_block(&name, self.pieces[start].line));
                }
                (Item::Rule { body }, Some(name)) if GROUPS.contains(&name.as_str()) => {
                    self.body(body + 1, end - 1)?;
                }
                (Item::Rule { body }, name) => self.rule(start, body, end, name)?,
            }
        }
        Ok(())
    }

    fn declaration(&self, start: usize, end: usize) -> Result<(), CssError> {
        let first = &self.pieces[start];
        if matches!(first.kind, Kind::Semicolon) {
            return Ok(());
        }
        let name_at = match first.kind {
            Kind::Delim('*') => start + 1,
            _ => start,
        };
        let name = match self.pieces.get(name_at).map(|piece| &piece.kind) {
            Some(Kind::Ident(name)) if matches!(after(self.pieces, name_at), Some(Kind::Colon)) => {
                name
            }
            _ => {
                return Err(CssError::at(
                    first.line,
                    format!(
                        "«{}» — не свойство и не правило со скобками {{ }}",
                        shown(self.text(start, end))
                    ),
                ));
            }
        };
        if name.starts_with("--") || name.eq_ignore_ascii_case("filter") {
            return Ok(());
        }
        let mut depth = 0usize;
        let mut seen: Option<usize> = None;
        for at in name_at..end {
            let kind = &self.pieces[at].kind;
            if kind.opens() {
                depth += 1;
            } else if kind.closes() {
                depth -= 1;
            } else if depth == 0 && matches!(kind, Kind::Colon) {
                match seen {
                    None => seen = Some(at),
                    Some(first) => return Err(self.no_semicolon(first, at)),
                }
            }
        }
        Ok(())
    }
}

struct Scoper<'a> {
    rules: Rules<'a>,
    classes: &'a [&'a str],
    scope: String,
    out: String,
}

impl<'a> Scoper<'a> {
    fn items(&mut self, from: usize, to: usize) -> Result<(), CssError> {
        let mut walk = Walk::new(self.rules.pieces, from, to);
        let mut own: Vec<&'a str> = Vec::new();
        while let Some((start, end, item)) = walk.next() {
            let line = self.rules.pieces[start].line;
            let at_rule = self.rules.at_name(start);
            if let (Item::Statement, None) = (item, &at_rule) {
                self.rules.declaration(start, end)?;
                let last = match self.rules.pieces[end - 1].kind {
                    Kind::Semicolon => end - 1,
                    _ => end,
                };
                let text = self.rules.text(start, last).trim_end();
                if !text.is_empty() {
                    own.push(text);
                }
                continue;
            }
            self.own(&mut own);
            match (item, at_rule) {
                (Item::Rule { body }, Some(name)) => self.at_rule(&name, start, body, end)?,
                (Item::Rule { body }, None) => self.rule(start, body, end)?,
                (Item::Statement, Some(name)) if name == "layer" => {
                    self.line(self.rules.text(start, end));
                }
                (Item::Statement, Some(name)) => return Err(no_block(&name, line)),
                (Item::Statement, None) => {}
            }
        }
        self.own(&mut own);
        Ok(())
    }

    fn line(&mut self, text: &str) {
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn own(&mut self, declarations: &mut Vec<&'a str>) {
        if declarations.is_empty() {
            return;
        }
        let body: Vec<String> = declarations
            .drain(..)
            .map(|text| format!("  {text};"))
            .collect();
        let rule = format!("{} {{\n{}\n}}", self.scope, body.join("\n"));
        self.line(&rule);
    }

    fn at_rule(
        &mut self,
        name: &str,
        start: usize,
        body: usize,
        end: usize,
    ) -> Result<(), CssError> {
        if GROUPS.contains(&name) {
            let prelude = self.rules.text(start, body).trim_end().to_string();
            self.line(&format!("{prelude} {{"));
            self.items(body + 1, end - 1)?;
            self.line("}");
            return Ok(());
        }
        if PAGE_WIDE.contains(&name) {
            return Err(CssError::at(
                self.rules.pieces[start].line,
                format!("@{name} действует на всю страницу: перенесите правило в «Общий CSS»"),
            ));
        }
        self.rules.rule(start, body, end, Some(name.to_string()))?;
        self.line(self.rules.text(start, end));
        Ok(())
    }

    fn rule(&mut self, start: usize, body: usize, end: usize) -> Result<(), CssError> {
        self.rules.rule(start, body, end, None)?;
        let mut selectors = Vec::new();
        let mut from = start;
        let mut depth = 0usize;
        for at in start..=body {
            let kind = &self.rules.pieces[at].kind;
            if at == body || (depth == 0 && matches!(kind, Kind::Comma)) {
                selectors.push(self.selector(from, at)?);
                from = at + 1;
            } else if kind.opens() {
                depth += 1;
            } else if kind.closes() {
                depth -= 1;
            }
        }
        let body = self.rules.text(body, end);
        let rule = format!("{} {body}", selectors.join(",\n"));
        self.line(&rule);
        Ok(())
    }

    fn selector(&self, from: usize, to: usize) -> Result<String, CssError> {
        let Rules { source, pieces } = self.rules;
        let blank = |at: &usize| pieces[*at].kind.blank();
        let Some(start) = (from..to).find(|at| !blank(at)) else {
            return Err(no_selector(pieces[from].line));
        };
        let end = (from..to)
            .rev()
            .find(|at| !blank(at))
            .map_or(to, |at| at + 1);
        let parts = &pieces[start..end];
        if parts
            .iter()
            .any(|piece| matches!(piece.kind, Kind::Delim('&')))
        {
            return Ok(parts
                .iter()
                .map(|piece| match piece.kind {
                    Kind::Delim('&') => self.scope.as_str(),
                    _ => &source[piece.start..piece.end],
                })
                .collect());
        }
        let mut inner = start;
        let mut page_wide = false;
        while let Some(wide) = self.outside(inner, end) {
            page_wide |= wide;
            inner = self.after_compound(inner, end);
        }
        if inner == end && page_wide {
            return Err(CssError::at(
                pieces[start].line,
                format!(
                    "«{}» действует на всю страницу: перенесите правило в «Общий CSS»",
                    shown(self.rules.text(start, end))
                ),
            ));
        }
        let outer = self.rules.text(start, inner).trim_end();
        let lead = if outer.is_empty() {
            self.scope.clone()
        } else {
            format!("{outer} {}", self.scope)
        };
        let rest = self.rules.text(inner, end);
        if rest.is_empty() {
            return Ok(lead);
        }
        let mut out = format!("{lead} {rest}");
        if matches!(
            pieces[inner].kind,
            Kind::Delim('.') | Kind::Open('[') | Kind::Other
        ) {
            out.push_str(&format!(",\n{lead}{rest}"));
        }
        Ok(out)
    }

    fn outside(&self, at: usize, end: usize) -> Option<bool> {
        if at >= end {
            return None;
        }
        let pieces = self.rules.pieces;
        let next = pieces.get(at + 1).map(|piece| &piece.kind);
        match (&pieces[at].kind, next) {
            (Kind::Ident(name), _) if is_page_tag(name) => Some(true),
            (Kind::Colon, Some(Kind::Ident(name))) if name.eq_ignore_ascii_case("root") => {
                Some(true)
            }
            (Kind::Delim('.'), Some(Kind::Ident(name))) => {
                self.classes.contains(&name.as_str()).then_some(false)
            }
            (Kind::Open('['), Some(Kind::Ident(name))) => {
                PAGE_ATTRIBUTES.contains(&name.as_str()).then_some(false)
            }
            _ => None,
        }
    }

    fn after_compound(&self, at: usize, end: usize) -> usize {
        let pieces = self.rules.pieces;
        let mut depth = 0usize;
        let mut next = at;
        while next < end {
            let kind = &pieces[next].kind;
            if depth == 0 && kind.blank() {
                break;
            }
            if kind.opens() {
                depth += 1;
            } else if kind.closes() {
                depth -= 1;
            }
            next += 1;
        }
        while next < end && pieces[next].kind.blank() {
            next += 1;
        }
        next
    }
}

fn is_page_tag(name: &str) -> bool {
    name.eq_ignore_ascii_case("html") || name.eq_ignore_ascii_case("body")
}

fn no_block(name: &str, line: u32) -> CssError {
    CssError::at(line, format!("после @{name} нет блока в скобках {{ }}"))
}

fn no_selector(line: u32) -> CssError {
    CssError::at(line, "перед скобкой { нет селектора")
}

pub fn clean(raw: &str) -> String {
    let unified = raw
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\u{c}', " ");
    let kept: String = unified
        .chars()
        .filter(|c| matches!(c, '\n' | '\t') || !c.is_control())
        .filter(|c| !matches!(c, '\u{200b}' | '\u{2060}' | '\u{feff}'))
        .map(|c| match c {
            '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}' => ' ',
            c => c,
        })
        .collect();
    kept.trim_end().to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckedCss {
    pub text: String,
    pub style: String,
    pub images: BTreeMap<String, u32>,
}

pub fn check<P: Place>(place: P, raw: &str) -> Result<CheckedCss, CssError> {
    let text = clean(raw);
    if text.trim().is_empty() {
        return Ok(CheckedCss::default());
    }
    if text.chars().count() > place.limit() {
        return Err(CssError {
            line: None,
            reason: format!("не длиннее {} символов", place.limit()),
        });
    }
    let source = format!("{text}\n");
    let pieces = read(&source)?;
    let images = check_pieces(&pieces, &P::SITE)?;
    let rules = Rules {
        source: &source,
        pieces: &pieces,
    };
    let style = match place.scope() {
        None => {
            rules.sheet(0, pieces.len())?;
            source
        }
        Some(scope) => {
            let mut scoper = Scoper {
                rules,
                classes: P::SITE.classes,
                scope,
                out: String::new(),
            };
            scoper.items(0, pieces.len())?;
            scoper.out
        }
    };
    Ok(CheckedCss {
        text,
        style,
        images,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PlaceError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedCss<P>(BTreeMap<P, String>);

impl<P> Default for PlacedCss<P> {
    fn default() -> Self {
        PlacedCss(BTreeMap::new())
    }
}

impl<P: Place> Serialize for PlacedCss<P> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, P: Place> Deserialize<'de> for PlacedCss<P> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = BTreeMap::<String, Option<String>>::deserialize(deserializer)?;
        Ok(PlacedCss(
            raw.into_iter()
                .filter_map(|(place, text)| {
                    Some((P::parse(&place)?, text.filter(|t| !t.is_empty())?))
                })
                .collect(),
        ))
    }
}

impl<P: Place> PlacedCss<P> {
    pub fn parse(raw: &BTreeMap<String, Option<String>>) -> Result<PlacedCss<P>, PlaceError> {
        let mut places = BTreeMap::new();
        for (name, text) in raw {
            let Some(place) = P::parse(name) else {
                return Err(PlaceError(format!(
                    "CSS: неизвестное место «{}»",
                    shown(name)
                )));
            };
            let checked = check(place, text.as_deref().unwrap_or_default())
                .map_err(|err| PlaceError(format!("{}, {err}", place.title())))?;
            if !checked.text.is_empty() {
                places.insert(place, checked.text);
            }
        }
        Ok(PlacedCss(places))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, place: P) -> &str {
        self.0.get(&place).map_or("", String::as_str)
    }

    pub fn places(&self) -> impl Iterator<Item = (P, &str)> {
        self.0.iter().map(|(place, text)| (*place, text.as_str()))
    }

    pub fn size(&self) -> usize {
        self.0.values().map(|text| text.chars().count()).sum()
    }

    pub fn changed(&self, next: &PlacedCss<P>) -> Vec<&'static str> {
        P::ALL
            .iter()
            .copied()
            .filter(|place| self.get(*place) != next.get(*place))
            .map(Place::as_str)
            .collect()
    }

    fn checked(&self) -> impl Iterator<Item = (P, CheckedCss)> {
        self.places()
            .filter_map(|(place, text)| Some((place, check(place, text).ok()?)))
    }

    pub fn styles(&self) -> BTreeMap<P, String> {
        self.checked()
            .map(|(place, checked)| (place, checked.style))
            .collect()
    }

    pub fn style(&self) -> String {
        self.checked().map(|(_, checked)| checked.style).collect()
    }

    pub fn images(&self) -> Vec<(P, u32, String)> {
        self.checked()
            .flat_map(|(place, checked)| {
                checked
                    .images
                    .into_iter()
                    .map(move |(tag, line)| (place, line, tag))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::landing::BlockKind;
    use crate::landing_css::{CssPlace, LandingCss};
    use crate::miniapp_css::{self, MiniCss};

    const TAG: &str = "0123456789abcdef";

    fn page(css: &str) -> Result<CheckedCss, CssError> {
        check(CssPlace::Page, css)
    }

    fn refused(css: &str) -> (u32, String) {
        let err = page(css).expect_err(css);
        (err.line.unwrap_or_default(), err.reason)
    }

    fn accepted(css: &str) {
        if let Err(err) = page(css) {
            panic!("{css}: {err}");
        }
    }

    #[test]
    fn обычный_css_проходит_без_изменений() {
        let css = ".ld {\n  --accent: #0f766e;\n}\n\n.ld-hero h1,\n.ld-card:nth-child(2n + 1) > a[href^=\"/cabinet\"]::before {\n  content: \"\\201C\";\n  font: 800 clamp(40px, 6vw, 92px)/1 system-ui;\n  background: linear-gradient(90deg, rgb(0 0 0 / 40%), transparent);\n  scroll-behavior: smooth;\n}\n\n.behavior:hover {\n  color: red !important;\n}\n\n@media (400px <= width <= 900px) and (prefers-reduced-motion: no-preference) {\n  .ld-card {\n    animation: appear 0.4s ease both;\n    &:hover {\n      color: blue;\n    }\n  }\n}\n\n@keyframes appear {\n  from { opacity: 0; }\n}\n\n@layer base, theme;\n@property --angle { syntax: \"<angle>\"; inherits: false; initial-value: 0deg; }\n@supports selector(:has(a)) { .ld:has(.ld-hero) { gap: 0 } }\n@font-face { font-family: \"Своя\"; src: local(\"Inter\"), url(data:font/woff2;base64,d09GMg==) format(\"woff2\"); }";
        let checked = page(css).unwrap();
        assert_eq!(checked.text, css);
        assert_eq!(checked.style, format!("{css}\n"));
        assert!(checked.images.is_empty());
        assert_eq!(page("").unwrap(), CheckedCss::default());
        assert_eq!(page("  \n\t\n").unwrap().text, "");
        accepted("/* @import \"x\"; javascript: behavior: expression( */ .a{}");
        accepted(".a::after { content: \"@import { не закрыто\"; }");
    }

    #[test]
    fn текст_чистится_от_служебных_символов_а_номера_строк_не_сдвигаются() {
        assert_eq!(
            clean("\u{feff}\r\n.a {\r\n\tcolor:\u{0}\u{8}\u{c}red;\r}\u{c}\n\n  "),
            "\n.a {\n\tcolor: red;\n}"
        );
        assert_eq!(clean("  .a {}"), "  .a {}");
        assert_eq!(clean(" \r\n\t "), "");
        assert_eq!(
            clean("\u{feff}.a {\n\u{a0}\u{2003}\u{200b}gap:\u{3000}0;\n}"),
            ".a {\n  gap: 0;\n}"
        );
        assert!(page(".a {\n\u{a0}\u{a0}color: red;\n}").is_ok());
    }

    #[test]
    fn незакрытые_скобки_кавычки_и_комментарии_называются_со_строкой() {
        assert_eq!(
            refused(".a {\n  color: red;\n\n.b {\n  color: blue;\n}\n"),
            (1, "не закрыта скобка {".into())
        );
        assert_eq!(
            refused("\n\n@media (width > 1px) {\n  .a { color: red; }\n"),
            (3, "не закрыта скобка {".into())
        );
        assert_eq!(
            refused(".a { color: red; }\n}\n.b {}"),
            (2, "лишняя скобка }".into())
        );
        assert_eq!(
            refused(".a {\n  color: rgb(1, 2, 3;\n}\n.b {}"),
            (2, "не закрыта скобка (".into())
        );
        assert_eq!(
            refused(".a {\n  width: calc(1px + 2px));\n}"),
            (2, "лишняя скобка )".into())
        );
        assert_eq!(refused("a[href {\n}"), (1, "не закрыта скобка [".into()));
        assert_eq!(
            refused(".a {\n  content: \"раз;\n}"),
            (2, "не закрыта кавычка".into())
        );
        assert_eq!(
            refused(".a { content: 'x"),
            (1, "не закрыта кавычка".into())
        );
        assert_eq!(
            refused(".a { content: 'x\\'"),
            (1, "не закрыта кавычка".into())
        );
        for css in ["--x: '\\", "--x: \"a\\", "a: 'b\\\\\\"] {
            assert_eq!(
                block_refused(css),
                (1, "не закрыта кавычка".into()),
                "{css}"
            );
        }
        assert_eq!(
            refused(".a {}\n/* заметка\n.b {}"),
            (2, "не закрыт комментарий /*".into())
        );
        assert_eq!(refused(".a {} /*/"), (1, "не закрыт комментарий /*".into()));
        assert_eq!(
            refused(".a { background: url(/bg.png"),
            (1, "не закрыта скобка (".into())
        );
        assert_eq!(
            refused(".a { background: url(/bg.png\\)"),
            (1, "не закрыта скобка (".into())
        );
        assert_eq!(
            refused(".a { background: url(\"/bg.png\" }"),
            (1, "не закрыта скобка (".into())
        );
        assert_eq!(
            refused(&format!(".a {{ width: {} }}", "(".repeat(40_000))).1,
            "слишком много вложенных скобок"
        );
        assert_eq!(
            refused(&"{".repeat(40_000)).1,
            "слишком много вложенных скобок"
        );
    }

    #[test]
    fn правила_без_селектора_или_блока_отклоняются() {
        assert_eq!(
            refused(".a {}\ncolor: red;"),
            (
                2,
                "«color: red;» стоит вне правила: нужен селектор и скобки { }".into()
            )
        );
        assert_eq!(
            refused("$main: #fff;\n.a {}").1,
            "«$main: #fff;» стоит вне правила: нужен селектор и скобки { }"
        );
        assert_eq!(
            refused(".a {}\n\n.b"),
            (
                3,
                "«.b» стоит вне правила: нужен селектор и скобки { }".into()
            )
        );
        assert_eq!(
            refused("{ color: red; }"),
            (1, "перед скобкой { нет селектора".into())
        );
        assert_eq!(
            refused("@media (width > 1px);"),
            (1, "после @media нет блока в скобках { }".into())
        );
        assert_eq!(
            refused(".a {};\n.b {}"),
            (1, "лишняя точка с запятой".into())
        );
        assert_eq!(
            refused("@media print {\n  .a {};\n  .b {}\n}"),
            (2, "лишняя точка с запятой".into())
        );
        assert_eq!(refused("@media print {\n  color: red;\n}").0, 2);
        let long = format!("{}: red;", "я".repeat(200));
        let (_, reason) = refused(&long);
        assert!(
            reason.starts_with(&format!("«{}…»", "я".repeat(60))),
            "{reason}"
        );
    }

    #[test]
    fn частые_ошибки_называются_понятно() {
        for (css, line, reason) in [
            (
                ".a {\n  color: red\n  background: blue;\n}",
                2,
                "похоже, пропущена точка с запятой",
            ),
            (
                ".a {\n  color:\n    red\n  gap: 0\n}",
                3,
                "похоже, пропущена точка с запятой",
            ),
            (
                ".a {\n  color: red\n  &:hover { gap: 0 }\n}",
                2,
                "похоже, пропущена точка с запятой",
            ),
            (
                "@media print {\n  .a { .b { gap: 0 margin: 0 } }\n}",
                2,
                "похоже, пропущена точка с запятой",
            ),
            (
                "@font-face {\n  font-family: A\n  src: local(B)\n}",
                2,
                "похоже, пропущена точка с запятой",
            ),
            (
                ".a { font: { size: 1px; } }",
                1,
                "лишнее двоеточие перед скобкой {",
            ),
            (
                ".a {}\n.b:{ gap: 0 }",
                2,
                "лишнее двоеточие перед скобкой {",
            ),
            (
                "@keyframes x {\n  from {\n    color: red\n    gap: 0\n  }\n}",
                3,
                "похоже, пропущена точка с запятой",
            ),
            (
                "@scope (.a) {\n  .b { color: red\n gap: 0 }\n}",
                2,
                "похоже, пропущена точка с запятой",
            ),
            (
                ".a {\n  color red;\n}",
                2,
                "«color red;» — не свойство и не правило со скобками { }",
            ),
            (
                ".a { .b }",
                1,
                "«.b» — не свойство и не правило со скобками { }",
            ),
            (".a { { gap: 0 } }", 1, "перед скобкой { нет селектора"),
            (
                ".a { @media print; }",
                1,
                "после @media нет блока в скобках { }",
            ),
            (
                "// заметка\n.a {}",
                1,
                "комментарий пишется так: /* текст */",
            ),
            (
                ".a {\n  gap: 0; // заметка\n}",
                2,
                "комментарий пишется так: /* текст */",
            ),
            (".a { color: red\\\n; gap: 0 }", 1, "лишний символ \\"),
            (
                ".a { content: “x” }",
                1,
                "замените кавычки на прямые: \"текст\" или 'текст'",
            ),
            (
                ".a { font-family: «Onest» }",
                1,
                "замените кавычки на прямые: \"текст\" или 'текст'",
            ),
        ] {
            assert_eq!(refused(css), (line, reason.into()), "{css}");
        }
        for css in [
            ".a { filter: progid:DXImageTransform.Microsoft.Alpha(opacity=50); *zoom: 1; _height: 1px }",
            ".a { --x: a:b; --y:; --z: { c: d } }",
            ".a { color: red; ; gap: 0 }",
            ".a:hover, a:not( :focus ):is(.b, .c) > p:first-child::before { gap: 0 }",
            "a:/* */hover { gap: 0 }",
            "@keyframes x { 0%, 50% { gap: 0 } to { gap: 1px } }",
            "@-moz-keyframes x { from { gap: 0 } }",
            "@page { margin: 1cm }",
            "@view-transition { navigation: auto }",
            "@scope (.a) to (.b) { :scope { gap: 0 } }",
            ".a { grid-area: 1 / 2 / 3; aspect-ratio: 16/9; content: \"// “x”\" }",
            ".a { background: if(style(--x: 1): red; else: blue) }",
            ".a { @media (width > 1px) { color: red; & b { gap: 0 } } @container (width > 1px) { gap: 0 } }",
            "@layer a { @layer b, c; .x { gap: 0 } }",
            "@font-face { font-family: A; src: local(B); unicode-range: U+0000-00FF, U+0131; }",
            ".a { width: calc(1px + (2px * 3)); transform: translate(0, -50%) rotate(45deg) }",
            ".\\31 23\\:hover { gap: 0 }",
        ] {
            accepted(css);
        }
    }

    #[test]
    fn чужие_файлы_не_подключаются_никаким_способом() {
        for css in [
            "@import \"https://evil.example/x.css\";",
            "@IMPORT url(x.css);",
            "@\\69mport 'x.css';",
            "@\\000069 mport 'x.css';",
            "@media all { @import 'x.css'; }",
            ".a { @import 'x.css'; }",
            "/**/@import 'x.css';",
        ] {
            assert_eq!(
                refused(css).1,
                "@import запрещён: скопируйте правила из того файла сюда",
                "{css}"
            );
        }
        assert_eq!(
            refused("@charset \"utf-8\";\n.a {}").1,
            "@charset не нужен, уберите эту строку"
        );
        for (css, name) in [
            (
                "@namespace svg url(http://www.w3.org/2000/svg);",
                "namespace",
            ),
            ("@document url(https://a.example) { .a {} }", "document"),
            ("@-moz-document url-prefix() { .a {} }", "-moz-document"),
            ("@include button;", "include"),
            (".a { @apply flex; }", "apply"),
        ] {
            assert_eq!(
                refused(css).1,
                format!("правило @{name} здесь нельзя"),
                "{css}"
            );
        }
    }

    #[test]
    fn скрипты_и_старые_обходы_запрещены() {
        for css in [
            ".a { background: url(javascript:alert(1)) }",
            ".a { background: url(javascript:alert) }",
            ".a { background: url(\"java\\0script:alert\") }",
            ".a { background: url('JavaScript:alert(1)') }",
            ".a { background: url(\"java\\\nscript:alert(1)\") }",
            ".a { background: url('\\6a avascript:alert(1)') }",
            ".a { background: url('java\\9 script:alert(1)') }",
            ".a { background: url(' vbscript:x') }",
            ".a { content: \"javascript:alert(1)\" }",
            ".a { --x: javascript:alert }",
            ".a { x: j\\61vascript:alert }",
            "a[href^=\"javascript:\"] { color: red }",
        ] {
            let (_, reason) = refused(css);
            assert!(
                reason == "ссылки javascript: и vbscript: в CSS запрещены"
                    || reason == "адрес в url() написан с ошибкой: возьмите его в кавычки"
                    || reason.starts_with("адрес «java\u{fffd}script"),
                "{css}: {reason}"
            );
        }
        for css in [
            ".a { width: expression(alert(1)) }",
            ".a { width: EXPRESSION(1) }",
            ".a { width: expr\\65ssion(1) }",
            ".a { --x: expression(1) }",
        ] {
            assert_eq!(refused(css).1, "expression() запрещён", "{css}");
        }
        for (css, name) in [
            (".a { behavior: url(#x) }", "behavior"),
            (".a { color: red; BEHAVIOR : url(#x) }", "behavior"),
            (".a { b\\65havior: url(#x) }", "behavior"),
            (".a { /* */ behavior /* */ : none }", "behavior"),
            (".a { .b {} behavior: none }", "behavior"),
            (".a { -moz-binding: url(#x) }", "-moz-binding"),
            (".a { -MOZ-binding: url(#x) }", "-moz-binding"),
            (".a { -ms-behavior: url(#x) }", "-ms-behavior"),
        ] {
            assert_eq!(
                refused(css).1,
                format!("свойство {name} запрещено"),
                "{css}"
            );
        }
    }

    #[test]
    fn адреса_только_свои_картинки_и_данные() {
        let own = format!("/api/public/landing/{TAG}.png");
        let checked = page(&format!(
            ".a {{ background: url({own}), url('/api/public/landing/fedcba9876543210.webp') }}\n.b {{ background: URL( \"{own}\" ) }}"
        ))
        .unwrap();
        assert_eq!(
            checked.images.into_iter().collect::<Vec<_>>(),
            [(TAG.to_string(), 1), ("fedcba9876543210".into(), 1)]
        );
        for css in [
            ".a { mask: url(#shape) }",
            ".a { background: url(/static/bg.png) }",
            ".a { background: url(\"/img/fon-2_1.webp\") }",
            ".a { src: url(/fonts/inter.woff2) format('woff2') }",
            ".a { background: url(data:image/png;base64,iVBORw0KGgo=) }",
            ".a { background: url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'/%3E\") }",
            ".a { src: url(DATA:application/font-woff2;base64,AAAA) }",
            ".a { background: image-set(url(/a.png) 1x, url(\"/b.png\") type(\"image/png\") 2x, linear-gradient(red, blue) 3x) }",
        ] {
            accepted(css);
            assert!(page(css).unwrap().images.is_empty(), "{css}");
        }
        for url in [
            "https://evil.example/x.png",
            "HTTP://evil.example/x.png",
            "//evil.example/x.png",
            "\\/\\/evil.example/x.png",
            "\\2f\\2f evil.example/x.png",
            "ftp://evil.example/x.png",
            "blob:https://example.com/1",
            "file:///etc/passwd",
            "ht\\tps://evil.example/x.png",
        ] {
            for css in [
                format!(".a {{ background: url(\"{url}\") }}"),
                format!(".a {{ background: url({url}) }}"),
                format!(".a {{ --x: url(\"{url}\") }}"),
                format!(".a {{ cursor: u\\72l({url}), auto }}"),
                format!("@font-face {{ src: url('{url}') }}"),
            ] {
                let (_, reason) = refused(&css);
                assert!(
                    reason.contains("только со своего домена"),
                    "{css}: {reason}"
                );
            }
        }
        for url in [
            "bg.png",
            "./bg.png",
            "../bg.png",
            "static/bg.png",
            "?x.png",
            "/\\evil.example/x.png",
            "/\\9 /evil.example/x.png",
            "/a//b.png",
            "/a/../b.png",
            "/a b.png",
            "/a.png?x=1",
            "/a.png#x",
            "/cabinet/a.png",
            "/admin/a.png",
            "/api/client/me.png",
            "/api/public/landing/../../x.png",
            "/api/public/landing/0123456789ABCDEF.png",
            "/api/public/landing/0123456789abcdef.svg",
            "/api/public/m/broadcast/unsubscribe/token",
            "/m/nodes/token/a.png",
            "/logout",
            "/.png",
            "/a.css",
            "/a.js",
            "/a.html",
        ] {
            let css = format!(".a {{ background: url(\"{url}\") }}");
            let (_, reason) = refused(&css);
            assert!(
                reason.contains("нужна картинка или шрифт с этого сайта"),
                "{css}: {reason}"
            );
        }
        for url in [
            "data:text/html,<script>alert(1)</script>",
            "data:text/css,.a{}",
            "data:,x",
            "data:application/javascript,alert(1)",
        ] {
            assert_eq!(
                refused(&format!(".a {{ background: url(\"{url}\") }}")).1,
                "в data: можно вставить только картинку или шрифт",
                "{url}"
            );
        }
        for css in [
            ".a { background: url() }",
            ".a { background: url(\"\") }",
            ".a { background: url( ) }",
        ] {
            assert!(
                [
                    "в url() нет адреса",
                    "внутри url() должен быть один адрес в кавычках"
                ]
                .contains(&refused(css).1.as_str()),
                "{css}"
            );
        }
        assert_eq!(
            refused(".a { background: url(var(--x)) }").1,
            "адрес в url() написан с ошибкой: возьмите его в кавычки"
        );
        assert_eq!(
            refused(".a { background: url(\"/a.png\" \"/b.png\") }").1,
            "внутри url() должен быть один адрес в кавычках"
        );
    }

    #[test]
    fn адрес_нельзя_спрятать_в_другой_функции() {
        for css in [
            ".a { background: image-set(\"https://evil.example/x.png\" 1x) }",
            ".a { background: -webkit-image-set('https://evil.example/x.png' 1x) }",
            ".a { --u: \"https://evil.example/x\"; background: image-set(var(--u) 1x) }",
            ".a { background: IMAGE-SET(attr(data-x) 1x) }",
            ".a { --x: image-set(\"https://evil.example/x.png\" 1x) }",
        ] {
            assert_eq!(
                refused(css).1,
                "внутри image-set() адреса пишутся только через url()",
                "{css}"
            );
        }
        assert_eq!(
            refused(".a { background: src(\"https://evil.example/x.png\") }").1,
            "src() здесь нельзя, напишите url()"
        );
        assert_eq!(
            refused(".a { background: image(\"https://evil.example/x.png\", red) }").1,
            "image() здесь нельзя, напишите url()"
        );
    }

    fn block(css: &str) -> String {
        match check(CssPlace::Hero, css) {
            Ok(checked) => {
                assert_eq!(checked.text, clean(css));
                assert!(
                    check(CssPlace::Page, &checked.style).is_ok(),
                    "{}",
                    checked.style
                );
                checked.style
            }
            Err(err) => panic!("{css}: {err}"),
        }
    }

    fn block_refused(css: &str) -> (u32, String) {
        let err = check(CssPlace::Hero, css).expect_err(css);
        (err.line.unwrap_or_default(), err.reason)
    }

    #[test]
    fn правила_блока_действуют_только_внутри_него() {
        for (css, scoped) in [
            (
                "h1 { color: red; }",
                "[data-block=\"hero\"] h1 { color: red; }\n",
            ),
            (
                "h1,\n  p > a:is(:hover, :focus) ,\n.ld-lead{color:red}",
                "[data-block=\"hero\"] h1,\n[data-block=\"hero\"] p > a:is(:hover, :focus),\n[data-block=\"hero\"] .ld-lead,\n[data-block=\"hero\"].ld-lead {color:red}\n",
            ),
            (
                "#top [x=\"a,b\"] {}",
                "[data-block=\"hero\"] #top [x=\"a,b\"],\n[data-block=\"hero\"]#top [x=\"a,b\"] {}\n",
            ),
            ("> p {}", "[data-block=\"hero\"] > p {}\n"),
            ("& {}", "[data-block=\"hero\"] {}\n"),
            (
                "&:hover .a, .b & {}",
                "[data-block=\"hero\"]:hover .a,\n.b [data-block=\"hero\"] {}\n",
            ),
            ("*::before {}", "[data-block=\"hero\"] *::before {}\n"),
            (
                "padding: 120px 24px;\nbackground: #fff;\nh1 { margin: 0 }\ngap: 10px",
                "[data-block=\"hero\"] {\n  padding: 120px 24px;\n  background: #fff;\n}\n[data-block=\"hero\"] h1 { margin: 0 }\n[data-block=\"hero\"] {\n  gap: 10px;\n}\n",
            ),
            (
                "--x: { a: b };\nh1 {}",
                "[data-block=\"hero\"] {\n  --x: { a: b };\n}\n[data-block=\"hero\"] h1 {}\n",
            ),
            (
                "@media (max-width: 600px) {\n  gap: 0;\n  h1 { font-size: 40px; }\n}",
                "@media (max-width: 600px) {\n[data-block=\"hero\"] {\n  gap: 0;\n}\n[data-block=\"hero\"] h1 { font-size: 40px; }\n}\n",
            ),
            (
                "@supports (display: grid) { @container (width > 1px) { p {} } }",
                "@supports (display: grid) {\n@container (width > 1px) {\n[data-block=\"hero\"] p {}\n}\n}\n",
            ),
            (
                "@keyframes in { from { opacity: 0 } }\n@font-face { font-family: A; src: local(B) }",
                "@keyframes in { from { opacity: 0 } }\n@font-face { font-family: A; src: local(B) }\n",
            ),
            (
                "h1 { span { color: red } &:hover { color: blue } }",
                "[data-block=\"hero\"] h1 { span { color: red } &:hover { color: blue } }\n",
            ),
            (
                "content: \\;;gap: 0;",
                "[data-block=\"hero\"] {\n  content: \\;;\n  gap: 0;\n}\n",
            ),
            (
                "gap: 0 /* a;b */ ;\n/* x */ margin: 0",
                "[data-block=\"hero\"] {\n  gap: 0 /* a;b */;\n  margin: 0;\n}\n",
            ),
            (
                "h1 {};\n;p {}",
                "[data-block=\"hero\"] h1 {}\n[data-block=\"hero\"] p {}\n",
            ),
        ] {
            assert_eq!(block(css), scoped, "{css}");
        }
    }

    #[test]
    fn тема_и_телефон_в_css_блока_остаются_снаружи() {
        for (css, scoped) in [
            (
                "[data-theme='dark'] h1 {}",
                "[data-theme='dark'] [data-block=\"hero\"] h1 {}\n",
            ),
            (
                ".ld-phone .ld-lead {}",
                ".ld-phone [data-block=\"hero\"] .ld-lead,\n.ld-phone [data-block=\"hero\"].ld-lead {}\n",
            ),
            (
                "[data-theme=\"dark\"] .ld.ld-phone p {}",
                "[data-theme=\"dark\"] .ld.ld-phone [data-block=\"hero\"] p {}\n",
            ),
            (
                "html[data-theme='light'] h1 {}",
                "html[data-theme='light'] [data-block=\"hero\"] h1 {}\n",
            ),
            (
                "[data-theme='dark'] {}",
                "[data-theme='dark'] [data-block=\"hero\"] {}\n",
            ),
            (":root h1 {}", ":root [data-block=\"hero\"] h1 {}\n"),
        ] {
            assert_eq!(block(css), scoped, "{css}");
        }
        for css in [
            "body { margin: 0 }",
            "html {}",
            ":root { --x: 1 }",
            "BODY {}",
        ] {
            let (line, reason) = block_refused(css);
            assert_eq!(line, 1);
            assert!(
                reason.ends_with("действует на всю страницу: перенесите правило в «Общий CSS»"),
                "{css}: {reason}"
            );
        }
    }

    #[test]
    fn ошибки_в_css_блока_тоже_со_строкой() {
        assert_eq!(
            block_refused("h1 {\n  color: red;\n"),
            (1, "не закрыта скобка {".into())
        );
        assert_eq!(
            block_refused("gap: 0;\n.ld-lead"),
            (
                2,
                "«.ld-lead» — не свойство и не правило со скобками { }".into()
            )
        );
        assert_eq!(
            block_refused("\n{ color: red }"),
            (2, "перед скобкой { нет селектора".into())
        );
        assert_eq!(
            block_refused("h1, , p { color: red }"),
            (1, "перед скобкой { нет селектора".into())
        );
        assert_eq!(
            block_refused("h1, { color: red }"),
            (1, "перед скобкой { нет селектора".into())
        );
        assert_eq!(
            block_refused("@media print;"),
            (1, "после @media нет блока в скобках { }".into())
        );
        for name in ["scope", "page", "view-transition", "-ms-viewport"] {
            assert_eq!(
                block_refused(&format!("p {{}}\n@{name} {{ margin: 0 }}")),
                (
                    2,
                    format!("@{name} действует на всю страницу: перенесите правило в «Общий CSS»")
                )
            );
        }
        assert_eq!(
            block_refused("gap: 0\nh1 { margin: 0 }"),
            (1, "похоже, пропущена точка с запятой".into())
        );
        assert_eq!(
            block_refused("color: red\nbackground: blue;"),
            (1, "похоже, пропущена точка с запятой".into())
        );
        assert_eq!(
            block_refused("h1 { color: red\n gap: 0 }"),
            (1, "похоже, пропущена точка с запятой".into())
        );
        assert_eq!(
            block_refused("background: url(https://evil.example/a.png)").0,
            1
        );
        assert_eq!(
            block_refused("behavior: url(#x);"),
            (1, "свойство behavior запрещено".into())
        );
        assert_eq!(
            block_refused("p {}\n@media all { behavior: url(#x) }"),
            (2, "свойство behavior запрещено".into())
        );
    }

    #[test]
    fn css_хранится_по_местам_и_собирается_в_один_файл() {
        let raw = |places: &[(&str, Option<&str>)]| -> BTreeMap<String, Option<String>> {
            places
                .iter()
                .map(|(place, text)| (place.to_string(), text.map(str::to_string)))
                .collect()
        };
        let css = LandingCss::parse(&raw(&[
            ("plans", Some(".ld-card { gap: 0 }")),
            ("page", Some(".ld { --accent: red }  \n\n")),
            (
                "hero",
                Some(&format!("background: url(/api/public/landing/{TAG}.png)")),
            ),
            ("faq", Some("  \n ")),
            ("cta", None),
        ]))
        .unwrap();
        assert_eq!(css.get(CssPlace::Page), ".ld { --accent: red }");
        assert_eq!(css.get(CssPlace::Faq), "");
        assert_eq!(
            css.places().map(|(place, _)| place).collect::<Vec<_>>(),
            [CssPlace::Page, CssPlace::Hero, CssPlace::Plans]
        );
        assert_eq!(
            css.style(),
            format!(
                ".ld {{ --accent: red }}\n[data-block=\"hero\"] {{\n  background: url(/api/public/landing/{TAG}.png);\n}}\n[data-block=\"plans\"] .ld-card,\n[data-block=\"plans\"].ld-card {{ gap: 0 }}\n"
            )
        );
        assert_eq!(css.images(), [(CssPlace::Hero, 1, TAG.to_string())]);
        let stored = serde_json::to_value(&css).unwrap();
        assert_eq!(stored["page"], ".ld { --accent: red }");
        assert_eq!(stored.as_object().unwrap().len(), 3);
        assert_eq!(serde_json::from_value::<LandingCss>(stored).unwrap(), css);

        let loose: LandingCss = serde_json::from_value(serde_json::json!({
            "page": ".a {} @import 'x';",
            "hero": "h1 {}",
            "video": "x",
            "cta": null,
            "faq": ""
        }))
        .unwrap();
        assert_eq!(
            loose.places().map(|(place, _)| place).collect::<Vec<_>>(),
            [CssPlace::Page, CssPlace::Hero]
        );
        assert_eq!(loose.style(), "[data-block=\"hero\"] h1 {}\n");

        let fail = |places: &[(&str, Option<&str>)]| LandingCss::parse(&raw(places)).unwrap_err().0;
        assert_eq!(
            fail(&[("video", Some("x"))]),
            "CSS: неизвестное место «video»"
        );
        assert_eq!(
            fail(&[("page", Some(".a {\n"))]),
            "Общий CSS, строка 1: не закрыта скобка {"
        );
        assert_eq!(
            fail(&[("faq", Some("p {}\np { color: red"))]),
            "Вопросы и ответы, CSS блока, строка 2: не закрыта скобка {"
        );
        assert_eq!(
            fail(&[("hero", Some(&"я".repeat(10_001)))]),
            "Первый экран, CSS блока, не длиннее 10000 символов"
        );
        assert_eq!(
            fail(&[("page", Some(&"я".repeat(50_001)))]),
            "Общий CSS, не длиннее 50000 символов"
        );
        assert!(
            LandingCss::parse(&raw(&[(
                "page",
                Some(&format!("/*{}*/", "я".repeat(49_996)))
            )]))
            .is_ok()
        );
    }

    #[test]
    fn у_каждого_блока_есть_своё_место_для_css() {
        let blocks: Vec<BlockKind> = CssPlace::ALL.iter().filter_map(|p| p.block()).collect();
        assert_eq!(blocks, BlockKind::ALL);
        for place in CssPlace::ALL.iter().copied() {
            assert_eq!(CssPlace::parse(place.as_str()), Some(place));
            assert_eq!(
                serde_json::to_value(place).unwrap(),
                serde_json::json!(place.as_str())
            );
        }
        assert_eq!(CssPlace::parse("video"), None);
    }

    fn mini(place: miniapp_css::CssPlace, css: &str) -> Result<CheckedCss, CssError> {
        check(place, css)
    }

    #[test]
    fn css_мини_приложения_ограничен_своей_частью_экрана() {
        use miniapp_css::CssPlace as Mini;
        for (place, css, scoped) in [
            (
                Mini::Subscription,
                ".mn-sub { gap: 0 }",
                "[data-part=\"subscription\"] .mn-sub,\n[data-part=\"subscription\"].mn-sub { gap: 0 }\n",
            ),
            (
                Mini::Header,
                "background: teal",
                "[data-part=\"header\"] {\n  background: teal;\n}\n",
            ),
            (
                Mini::Tabs,
                "[data-theme='dark'] .mn .mn-tab[aria-current] { color: red }",
                "[data-theme='dark'] .mn [data-part=\"tabs\"] .mn-tab[aria-current],\n[data-theme='dark'] .mn [data-part=\"tabs\"].mn-tab[aria-current] { color: red }\n",
            ),
            (
                Mini::Profile,
                "@media (min-width: 400px) { &:hover .mn-card, .mn-row {} }",
                "@media (min-width: 400px) {\n[data-part=\"profile\"]:hover .mn-card,\n[data-part=\"profile\"] .mn-row,\n[data-part=\"profile\"].mn-row {}\n}\n",
            ),
            (
                Mini::Banners,
                ".ld { color: red }",
                "[data-part=\"banners\"] .ld,\n[data-part=\"banners\"].ld { color: red }\n",
            ),
            (
                Mini::Page,
                ".mn { --accent: teal }",
                ".mn { --accent: teal }\n",
            ),
        ] {
            assert_eq!(mini(place, css).unwrap().style, scoped, "{css}");
        }
        for css in ["html { margin: 0 }", "body {}", ":root { --bg: red }"] {
            let err = mini(Mini::Actions, css).unwrap_err();
            assert_eq!(err.line, Some(1));
            assert!(err.reason.contains("«Общий CSS»"), "{css}: {err}");
            assert!(mini(Mini::Page, css).is_ok(), "{css}");
        }
    }

    #[test]
    fn css_мини_приложения_берёт_только_свои_картинки_и_держит_те_же_запреты() {
        use miniapp_css::CssPlace as Mini;
        let own = format!("background: url(/api/public/miniapp/{TAG}.png)");
        let checked = mini(Mini::Banners, &own).unwrap();
        assert_eq!(checked.images.keys().collect::<Vec<_>>(), [TAG]);
        let foreign = format!("background: url(/api/public/landing/{TAG}.png)");
        let reason = mini(Mini::Banners, &foreign).unwrap_err().reason;
        assert!(
            reason.ends_with("например /api/public/miniapp/0123456789abcdef.png"),
            "{reason}"
        );
        assert!(check(CssPlace::Hero, &own).is_err());
        for css in [
            "@import 'https://evil.example/x.css';",
            ".mn { background: url(https://evil.example/x.png) }",
            ".mn { background: url(//evil.example/x.png) }",
            ".mn { background: image-set('https://evil.example/x.png' 1x) }",
            ".mn { background: url('javascript:alert(1)') }",
            ".mn { behavior: url(#x) }",
            "</style><script>alert(1)</script>",
        ] {
            assert!(mini(Mini::Page, css).is_err(), "{css}");
        }
        let long = |n: usize| format!("/*{}*/", "я".repeat(n));
        assert!(mini(Mini::Page, &long(49_996)).is_ok());
        assert_eq!(
            mini(Mini::Page, &long(50_000)).unwrap_err().reason,
            "не длиннее 50000 символов"
        );
        assert!(mini(Mini::Tabs, &long(9_996)).is_ok());
        assert_eq!(
            mini(Mini::Tabs, &long(10_000)).unwrap_err().reason,
            "не длиннее 10000 символов"
        );
    }

    #[test]
    fn места_css_мини_приложения_есть_в_его_разметке_и_в_админке() {
        use miniapp_css::CssPlace as Mini;
        let web = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/src");
        let mut markup = String::new();
        for entry in std::fs::read_dir(format!("{web}/mini")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|kind| kind == "tsx") {
                markup.push_str(&std::fs::read_to_string(path).unwrap());
            }
        }
        for place in Mini::ALL.iter().copied() {
            let name = place.as_str();
            assert_eq!(Mini::parse(name), Some(place));
            assert_eq!(
                serde_json::to_value(place).unwrap(),
                serde_json::json!(name)
            );
            assert!(
                place == Mini::Page || markup.contains(&format!("data-part=\"{name}\"")),
                "{name}"
            );
        }
        assert_eq!(Mini::parse("hero"), None);
        let source = std::fs::read_to_string(format!("{web}/admin/miniapp/css.ts")).unwrap();
        let list = source
            .split_once("MINI_PLACES: MiniPlace[] = [")
            .unwrap()
            .1
            .split_once(']')
            .unwrap()
            .0;
        let listed: Vec<&str> = list
            .split(',')
            .map(|name| name.trim().trim_matches('\''))
            .filter(|name| !name.is_empty())
            .collect();
        let places: Vec<&str> = Mini::ALL.iter().map(|place| place.as_str()).collect();
        assert_eq!(listed, places);

        let css = MiniCss::parse(&BTreeMap::from([
            ("tabs".to_string(), Some("gap: 0".to_string())),
            ("page".to_string(), Some(".mn { gap: 0 }".to_string())),
        ]))
        .unwrap();
        assert_eq!(
            css.style(),
            ".mn { gap: 0 }\n[data-part=\"tabs\"] {\n  gap: 0;\n}\n"
        );
        assert_eq!(css.size(), 20);
        assert_eq!(css.changed(&MiniCss::default()), ["page", "tabs"]);
        let fail = |place: &str, text: &str| {
            MiniCss::parse(&BTreeMap::from([(
                place.to_string(),
                Some(text.to_string()),
            )]))
            .unwrap_err()
            .0
        };
        assert_eq!(fail("hero", "x"), "CSS: неизвестное место «hero»");
        assert_eq!(
            fail("actions", "a {\n"),
            "Кнопки под подпиской, CSS блока, строка 1: не закрыта скобка {"
        );
        assert_eq!(
            fail("header", "a {\n"),
            "Шапка, CSS блока, строка 1: не закрыта скобка {"
        );
        assert_eq!(
            fail("tabs", "a {\n"),
            "CSS меню внизу, строка 1: не закрыта скобка {"
        );
        assert_eq!(
            fail("profile", "a {\n"),
            "CSS профиля, строка 1: не закрыта скобка {"
        );
    }

    #[test]
    fn разметка_html_в_css_не_принимается() {
        for css in [
            "<style>\n.a {}\n</style>",
            ".a {}\n</style><script>alert(1)</script>",
            ".a { color: red } <script>",
            ".a { color: red; </ style }",
            ".a {} <!DOCTYPE html>",
        ] {
            assert_eq!(
                refused(css).1,
                "уберите теги вроде <style>: нужен только CSS",
                "{css}"
            );
        }
        assert_eq!(
            refused("<!--\n.a {}\n-->").1,
            "уберите <!-- и -->: это разметка HTML"
        );
    }
}
