use serde::{Deserialize, Serialize};

pub const LOGO_MAX_BYTES: usize = 1024 * 1024;
pub const LOGO_MIN_SIDE: u32 = 32;
pub const LOGO_MAX_SIDE: u32 = 4096;
pub const FAVICON_MAX_BYTES: usize = 512 * 1024;
pub const FAVICON_MIN_SIDE: u32 = 16;
pub const FAVICON_MAX_SIDE: u32 = 1024;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const ICO_SIGNATURE: [u8; 4] = [0, 0, 1, 0];
const ICO_MAX_ENTRIES: usize = 32;
const DIB_HEADERS: [u32; 6] = [12, 40, 52, 56, 108, 124];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageKind {
    Png,
    Jpeg,
    Webp,
    Ico,
}

impl ImageKind {
    pub fn content_type(self) -> &'static str {
        match self {
            ImageKind::Png => "image/png",
            ImageKind::Jpeg => "image/jpeg",
            ImageKind::Webp => "image/webp",
            ImageKind::Ico => "image/x-icon",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ImageKind::Png => "png",
            ImageKind::Jpeg => "jpg",
            ImageKind::Webp => "webp",
            ImageKind::Ico => "ico",
        }
    }

    pub fn sniff(bytes: &[u8]) -> Option<ImageKind> {
        if bytes.starts_with(&PNG_SIGNATURE) {
            Some(ImageKind::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(ImageKind::Jpeg)
        } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some(ImageKind::Webp)
        } else if bytes.len() >= 6 && bytes.starts_with(&ICO_SIGNATURE) {
            Some(ImageKind::Ico)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    pub kind: ImageKind,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProblem {
    Empty,
    TooHeavy,
    Format,
    Damaged,
    TooSmall,
    TooLarge,
    NotSquare,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageRules {
    pub kinds: &'static [ImageKind],
    pub max_bytes: usize,
    pub min_side: u32,
    pub max_side: u32,
    pub square: bool,
}

pub const LOGO_RULES: ImageRules = ImageRules {
    kinds: &[ImageKind::Png, ImageKind::Jpeg, ImageKind::Webp],
    max_bytes: LOGO_MAX_BYTES,
    min_side: LOGO_MIN_SIDE,
    max_side: LOGO_MAX_SIDE,
    square: false,
};

pub const FAVICON_RULES: ImageRules = ImageRules {
    kinds: &[ImageKind::Png, ImageKind::Ico],
    max_bytes: FAVICON_MAX_BYTES,
    min_side: FAVICON_MIN_SIDE,
    max_side: FAVICON_MAX_SIDE,
    square: true,
};

pub fn inspect(bytes: &[u8]) -> Option<ImageInfo> {
    let kind = ImageKind::sniff(bytes)?;
    let (width, height) = match kind {
        ImageKind::Png => png_size(bytes),
        ImageKind::Jpeg => jpeg_size(bytes),
        ImageKind::Webp => webp_size(bytes),
        ImageKind::Ico => ico_size(bytes),
    }?;
    (width > 0 && height > 0).then_some(ImageInfo {
        kind,
        width,
        height,
    })
}

pub fn check(bytes: &[u8], rules: &ImageRules) -> Result<ImageInfo, ImageProblem> {
    if bytes.is_empty() {
        return Err(ImageProblem::Empty);
    }
    if bytes.len() > rules.max_bytes {
        return Err(ImageProblem::TooHeavy);
    }
    if !ImageKind::sniff(bytes).is_some_and(|kind| rules.kinds.contains(&kind)) {
        return Err(ImageProblem::Format);
    }
    let info = inspect(bytes).ok_or(ImageProblem::Damaged)?;
    if rules.square && info.width != info.height {
        return Err(ImageProblem::NotSquare);
    }
    if info.width < rules.min_side || info.height < rules.min_side {
        return Err(ImageProblem::TooSmall);
    }
    if info.width > rules.max_side || info.height > rules.max_side {
        return Err(ImageProblem::TooLarge);
    }
    Ok(info)
}

pub fn check_logo(bytes: &[u8]) -> Result<ImageInfo, ImageProblem> {
    check(bytes, &LOGO_RULES)
}

pub fn check_favicon(bytes: &[u8]) -> Result<ImageInfo, ImageProblem> {
    check(bytes, &FAVICON_RULES)
}

fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
    Some((width, height))
}

fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    loop {
        if *bytes.get(at)? != 0xFF {
            return None;
        }
        let mut marker_at = at + 1;
        while *bytes.get(marker_at)? == 0xFF {
            marker_at += 1;
        }
        let marker = *bytes.get(marker_at)?;
        at = marker_at + 1;
        match marker {
            0x01 | 0xD0..=0xD8 => continue,
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let length = usize::from(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]));
        if length < 2 {
            return None;
        }
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let height = u16::from_be_bytes([*bytes.get(at + 3)?, *bytes.get(at + 4)?]);
            let width = u16::from_be_bytes([*bytes.get(at + 5)?, *bytes.get(at + 6)?]);
            return Some((u32::from(width), u32::from(height)));
        }
        at += length;
    }
}

fn webp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    match bytes.get(12..16)? {
        b"VP8 " => {
            if bytes.get(23..26)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            let width = u16::from_le_bytes([*bytes.get(26)?, *bytes.get(27)?]) & 0x3FFF;
            let height = u16::from_le_bytes([*bytes.get(28)?, *bytes.get(29)?]) & 0x3FFF;
            Some((u32::from(width), u32::from(height)))
        }
        b"VP8L" => {
            if *bytes.get(20)? != 0x2F {
                return None;
            }
            let bits = u32::from_le_bytes(bytes.get(21..25)?.try_into().ok()?);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        b"VP8X" => {
            let raw = bytes.get(24..30)?;
            let width = u32::from_le_bytes([raw[0], raw[1], raw[2], 0]) + 1;
            let height = u32::from_le_bytes([raw[3], raw[4], raw[5], 0]) + 1;
            Some((width, height))
        }
        _ => None,
    }
}

fn ico_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let count = usize::from(u16::from_le_bytes([*bytes.get(4)?, *bytes.get(5)?]));
    if count == 0 || count > ICO_MAX_ENTRIES {
        return None;
    }
    let side = |raw: u8| if raw == 0 { 256 } else { u32::from(raw) };
    let mut best: Option<(u32, u32)> = None;
    for index in 0..count {
        let entry = bytes.get(6 + index * 16..6 + (index + 1) * 16)?;
        let size = usize::try_from(u32::from_le_bytes(entry[8..12].try_into().ok()?)).ok()?;
        let offset = usize::try_from(u32::from_le_bytes(entry[12..16].try_into().ok()?)).ok()?;
        let data = bytes.get(offset..offset.checked_add(size)?)?;
        let found = if data.starts_with(&PNG_SIGNATURE) {
            png_size(data)?
        } else {
            let header = u32::from_le_bytes(data.get(0..4)?.try_into().ok()?);
            if !DIB_HEADERS.contains(&header) {
                return None;
            }
            (side(entry[0]), side(entry[1]))
        };
        let area = |(w, h): (u32, u32)| u64::from(w) * u64::from(h);
        if best.is_none_or(|current| area(found) > area(current)) {
            best = Some(found);
        }
    }
    best
}

#[cfg(test)]
mod samples {
    pub fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        bytes
    }

    pub fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        bytes.extend_from_slice(b"JFIF\0");
        bytes.extend_from_slice(&[1, 1, 0, 0, 1, 0, 1, 0, 0]);
        bytes.extend_from_slice(&[0xFF, 0xFF, 0xC2, 0x00, 0x11, 0x08]);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    }

    pub fn ico(entries: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let count = u16::try_from(entries.len()).unwrap();
        let mut bytes = vec![0, 0, 1, 0];
        bytes.extend_from_slice(&count.to_le_bytes());
        let mut offset = 6 + 16 * entries.len();
        for (side, data) in entries {
            bytes.extend_from_slice(&[*side, *side, 0, 0, 1, 0, 32, 0]);
            bytes.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
            bytes.extend_from_slice(&u32::try_from(offset).unwrap().to_le_bytes());
            offset += data.len();
        }
        for (_, data) in entries {
            bytes.extend_from_slice(data);
        }
        bytes
    }

    pub fn dib() -> Vec<u8> {
        let mut bytes = 40u32.to_le_bytes().to_vec();
        bytes.resize(40, 0);
        bytes
    }

    pub fn webp_extended(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"RIFF\x1e\0\0\0WEBPVP8X\x0a\0\0\0\x10\0\0\0".to_vec();
        bytes.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
        bytes.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::samples::*;
    use super::*;

    #[test]
    fn размеры_читаются_из_заголовков_всех_трёх_форматов() {
        assert_eq!(
            inspect(&png(512, 256)),
            Some(ImageInfo {
                kind: ImageKind::Png,
                width: 512,
                height: 256
            })
        );
        assert_eq!(
            inspect(&jpeg(200, 128)),
            Some(ImageInfo {
                kind: ImageKind::Jpeg,
                width: 200,
                height: 128
            })
        );
        assert_eq!(
            inspect(&webp_extended(300, 120)),
            Some(ImageInfo {
                kind: ImageKind::Webp,
                width: 300,
                height: 120
            })
        );
        let mut lossless = b"RIFF\x1a\0\0\0WEBPVP8L\x0d\0\0\0\x2f".to_vec();
        let bits: u32 = (64 - 1) | ((48 - 1) << 14);
        lossless.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(
            inspect(&lossless).map(|i| (i.width, i.height)),
            Some((64, 48))
        );
        let mut lossy = b"RIFF\x1e\0\0\0WEBPVP8 \x12\0\0\0\0\0\0\x9d\x01\x2a".to_vec();
        lossy.extend_from_slice(&100u16.to_le_bytes());
        lossy.extend_from_slice(&80u16.to_le_bytes());
        assert_eq!(
            inspect(&lossy).map(|i| (i.width, i.height)),
            Some((100, 80))
        );
    }

    #[test]
    fn логотип_проверяется_по_содержимому_а_не_по_имени() {
        assert_eq!(check_logo(&[]), Err(ImageProblem::Empty));
        assert_eq!(
            check_logo(b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script/></svg>"),
            Err(ImageProblem::Format)
        );
        assert_eq!(check_logo(b"GIF89a\x40\0\x40\0"), Err(ImageProblem::Format));
        assert_eq!(check_logo(&png(512, 512)[..14]), Err(ImageProblem::Damaged));
        assert_eq!(check_logo(&png(16, 16)), Err(ImageProblem::TooSmall));
        assert_eq!(check_logo(&png(5000, 300)), Err(ImageProblem::TooLarge));
        let mut heavy = png(256, 256);
        heavy.resize(LOGO_MAX_BYTES + 1, 0);
        assert_eq!(check_logo(&heavy), Err(ImageProblem::TooHeavy));
        assert_eq!(
            check_logo(&png(256, 256)).map(|i| i.kind),
            Ok(ImageKind::Png)
        );
        assert_eq!(
            check_logo(&jpeg(256, 256)).map(|i| i.kind),
            Ok(ImageKind::Jpeg)
        );
        let jpeg_without_frame = [0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02, 0xFF, 0xD9];
        assert_eq!(check_logo(&jpeg_without_frame), Err(ImageProblem::Damaged));
    }

    #[test]
    fn значок_ico_читается_по_самому_крупному_изображению() {
        let single = ico(&[(32, dib())]);
        assert_eq!(
            inspect(&single),
            Some(ImageInfo {
                kind: ImageKind::Ico,
                width: 32,
                height: 32
            })
        );
        let several = ico(&[(16, dib()), (48, dib()), (0, png(256, 256))]);
        assert_eq!(
            inspect(&several).map(|i| (i.width, i.height)),
            Some((256, 256))
        );
        let mut broken = ico(&[(32, dib())]);
        broken.truncate(30);
        assert_eq!(inspect(&broken), None);
        let not_bitmap = ico(&[(32, vec![1, 2, 3, 4, 5, 6, 7, 8])]);
        assert_eq!(inspect(&not_bitmap), None);
        assert_eq!(inspect(&[0, 0, 1, 0, 0, 0]), None);
    }

    #[test]
    fn фавикон_только_квадратный_png_или_ico() {
        assert_eq!(
            check_favicon(&png(512, 512)).map(|i| i.kind),
            Ok(ImageKind::Png)
        );
        assert_eq!(
            check_favicon(&ico(&[(16, dib()), (32, dib())])).map(|i| i.width),
            Ok(32)
        );
        assert_eq!(check_favicon(&png(512, 256)), Err(ImageProblem::NotSquare));
        assert_eq!(check_favicon(&jpeg(256, 256)), Err(ImageProblem::Format));
        assert_eq!(check_favicon(&png(8, 8)), Err(ImageProblem::TooSmall));
        assert_eq!(check_favicon(&png(2048, 2048)), Err(ImageProblem::TooLarge));
        let mut heavy = png(256, 256);
        heavy.resize(FAVICON_MAX_BYTES + 1, 0);
        assert_eq!(check_favicon(&heavy), Err(ImageProblem::TooHeavy));
        let mut cut = ico(&[(32, dib())]);
        cut.truncate(40);
        assert_eq!(check_favicon(&cut), Err(ImageProblem::Damaged));
        assert_eq!(check_logo(&ico(&[(64, dib())])), Err(ImageProblem::Format));
    }

    #[test]
    fn у_каждого_формата_свой_тип_и_расширение() {
        assert_eq!(ImageKind::Ico.content_type(), "image/x-icon");
        assert_eq!(ImageKind::Ico.extension(), "ico");
        assert_eq!(ImageKind::Png.content_type(), "image/png");
        assert_eq!(ImageKind::Jpeg.extension(), "jpg");
        assert_eq!(ImageKind::Webp.content_type(), "image/webp");
    }
}
