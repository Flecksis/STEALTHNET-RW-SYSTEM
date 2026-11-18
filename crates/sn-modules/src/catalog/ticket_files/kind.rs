use sn_domain::support::FileKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileType {
    pub mime: &'static str,
    pub extension: &'static str,
    pub kind: FileKind,
}

pub const JPEG: FileType = FileType {
    mime: "image/jpeg",
    extension: "jpg",
    kind: FileKind::Image,
};
pub const PNG: FileType = FileType {
    mime: "image/png",
    extension: "png",
    kind: FileKind::Image,
};
pub const WEBP: FileType = FileType {
    mime: "image/webp",
    extension: "webp",
    kind: FileKind::Image,
};
pub const GIF: FileType = FileType {
    mime: "image/gif",
    extension: "gif",
    kind: FileKind::Image,
};
pub const PDF: FileType = FileType {
    mime: "application/pdf",
    extension: "pdf",
    kind: FileKind::File,
};
pub const TEXT: FileType = FileType {
    mime: "text/plain",
    extension: "txt",
    kind: FileKind::File,
};

pub const TYPES: [FileType; 6] = [JPEG, PNG, WEBP, GIF, PDF, TEXT];

const EXTENSIONS: [(&str, FileType); 8] = [
    ("jpg", JPEG),
    ("jpeg", JPEG),
    ("png", PNG),
    ("webp", WEBP),
    ("gif", GIF),
    ("pdf", PDF),
    ("txt", TEXT),
    ("log", TEXT),
];

pub const ALLOWED_TEXT: &str = "фото JPG, PNG, WebP, GIF и файлы PDF, TXT, LOG";

const MARKUP: [&str; 10] = [
    "<svg",
    "<html",
    "<!doctype",
    "<script",
    "<?xml",
    "<body",
    "<head",
    "<iframe",
    "<math",
    "<object",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Empty,
    Type,
}

pub fn by_mime(mime: &str) -> Option<FileType> {
    let mime = mime.split(';').next().unwrap_or("").trim();
    TYPES
        .into_iter()
        .find(|t| t.mime.eq_ignore_ascii_case(mime))
}

fn extension_of(name: &str) -> Option<String> {
    let (_, ext) = base_name(name).rsplit_once('.')?;
    let ext = ext.trim().to_ascii_lowercase();
    (!ext.is_empty()).then_some(ext)
}

pub fn by_extension(name: &str) -> Option<FileType> {
    let ext = extension_of(name)?;
    EXTENSIONS
        .into_iter()
        .find(|(known, _)| *known == ext)
        .map(|(_, t)| t)
}

pub fn allowed_name(name: &str) -> bool {
    extension_of(name).is_none_or(|ext| EXTENSIONS.iter().any(|(known, _)| *known == ext))
}

fn markup(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    let text = String::from_utf8_lossy(head).to_lowercase();
    let start = text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    start.starts_with('<') && MARKUP.iter().any(|tag| start.contains(tag))
}

fn plain_text(bytes: &[u8]) -> bool {
    if bytes.contains(&0) || markup(bytes) {
        return false;
    }
    let control = bytes
        .iter()
        .filter(|b| **b < 0x20 && !matches!(**b, b'\t' | b'\n' | b'\r' | 0x0c | 0x1b))
        .count();
    control * 100 <= bytes.len()
}

pub fn sniff(bytes: &[u8]) -> Option<FileType> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(JPEG);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(PNG);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(GIF);
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(WEBP);
    }
    let head = &bytes[..bytes.len().min(1024)];
    if head.windows(5).any(|w| w == b"%PDF-") {
        return Some(PDF);
    }
    plain_text(bytes).then_some(TEXT)
}

pub fn detect(name: &str, bytes: &[u8]) -> Result<FileType, Refusal> {
    if bytes.is_empty() {
        return Err(Refusal::Empty);
    }
    if !allowed_name(name) {
        return Err(Refusal::Type);
    }
    sniff(bytes).ok_or(Refusal::Type)
}

fn base_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

pub fn file_name(name: &str, file: FileType) -> String {
    let cleaned: String = base_name(name)
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '"' | '<' | '>' | '|' | '*' | '?' | ':') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let (stem, ext) = match cleaned.rsplit_once('.') {
        Some((stem, ext)) if !stem.trim().is_empty() => (stem.trim().to_string(), Some(ext)),
        _ => (cleaned.trim_matches('.').trim().to_string(), None),
    };
    let ext = ext
        .map(str::to_ascii_lowercase)
        .filter(|ext| by_extension(&format!("x.{ext}")) == Some(file))
        .unwrap_or_else(|| file.extension.to_string());
    let stem: String = stem
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .chars()
        .take(100)
        .collect();
    let stem = if stem.trim().is_empty() {
        match file.kind {
            FileKind::Image => "photo".to_string(),
            FileKind::File => "file".to_string(),
        }
    } else {
        stem.trim().to_string()
    };
    format!("{stem}.{ext}")
}
