use super::*;
use sn_domain::support::FileKind;

fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0xFF, marker];
    out.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn exif(orientation: u16) -> Vec<u8> {
    let mut tiff = b"II\x2a\0\x08\0\0\0".to_vec();
    tiff.extend_from_slice(&2u16.to_le_bytes());
    tiff.extend_from_slice(&0x0112u16.to_le_bytes());
    tiff.extend_from_slice(&3u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&orientation.to_le_bytes());
    tiff.extend_from_slice(&[0, 0]);
    tiff.extend_from_slice(&0x8825u16.to_le_bytes());
    tiff.extend_from_slice(&4u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&40u32.to_le_bytes());
    tiff.extend_from_slice(&0u32.to_le_bytes());
    tiff.extend_from_slice(b"GPSLatitude 55.7558 GPSLongitude 37.6173");
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&tiff);
    payload
}

pub fn jpeg_with_gps(orientation: u16) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8];
    out.extend(segment(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"));
    out.extend(segment(0xE1, &exif(orientation)));
    out.extend(segment(
        0xE1,
        b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>GPS 55.7558</x:xmpmeta>",
    ));
    out.extend(segment(0xED, b"Photoshop 3.0\0city=Moscow"));
    out.extend(segment(0xFE, b"comment with location"));
    out.extend(segment(0xE2, b"ICC_PROFILE\0\x01\x01profile"));
    out.extend(segment(0xDB, &[0u8; 65]));
    out.extend(segment(0xC0, &[8, 0x01, 0xE0, 0x02, 0x80, 1, 1, 0x11, 0]));
    out.extend(segment(0xDA, &[1, 1, 0, 0, 0x3F, 0]));
    out.extend_from_slice(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56]);
    out.extend_from_slice(&[0xFF, 0xD9]);
    out.extend_from_slice(b"trailing motion photo with GPSLatitude");
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out
}

pub fn png_with_text() -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = 640u32.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&480u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    out.extend(png_chunk(b"IHDR", &ihdr));
    out.extend(png_chunk(b"tEXt", b"Location\0Moscow"));
    out.extend(png_chunk(b"eXIf", b"MM\0*GPS"));
    out.extend(png_chunk(b"IDAT", b"pixels"));
    out.extend(png_chunk(b"IEND", b""));
    out
}

fn webp_with_exif() -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    body.extend_from_slice(b"VP8X");
    body.extend_from_slice(&10u32.to_le_bytes());
    body.extend_from_slice(&[0x0C, 0, 0, 0]);
    body.extend_from_slice(&[0x1F, 0x03, 0x00]);
    body.extend_from_slice(&[0xDF, 0x01, 0x00]);
    body.extend_from_slice(b"VP8L");
    body.extend_from_slice(&5u32.to_le_bytes());
    body.extend_from_slice(&[0x2f, 1, 2, 3, 4, 0]);
    body.extend_from_slice(b"EXIF");
    body.extend_from_slice(&7u32.to_le_bytes());
    body.extend_from_slice(b"GPS 55.\0");
    body.extend_from_slice(b"XMP ");
    body.extend_from_slice(&4u32.to_le_bytes());
    body.extend_from_slice(b"<x/>");
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
    out.extend(body);
    out
}

#[test]
fn типы_файлов_определяются_по_содержимому() {
    assert_eq!(sniff(&jpeg_with_gps(1)), Some(JPEG));
    assert_eq!(sniff(&png_with_text()), Some(PNG));
    assert_eq!(sniff(&webp_with_exif()), Some(WEBP));
    assert_eq!(sniff(b"GIF89a\x10\0\x10\0"), Some(GIF));
    assert_eq!(sniff(b"%PDF-1.7\n%..."), Some(PDF));
    assert_eq!(
        sniff("2026-11-18 ошибка подключения\n".as_bytes()),
        Some(TEXT)
    );
    assert_eq!(sniff(b"MZ\x90\0\x03\0\0\0"), None);
    assert_eq!(
        sniff(b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>"),
        None
    );
    assert_eq!(
        sniff(b"\xef\xbb\xbf  <!DOCTYPE html><html><body>hi</body></html>"),
        None
    );
    assert_eq!(sniff(b"<?xml version=\"1.0\"?><svg/>"), None);
    assert_eq!(sniff(b"PK\x03\x04zip"), None);
}

#[test]
fn запрещённые_расширения_и_подмена_типа_отсекаются() {
    assert_eq!(detect("screen.png", &png_with_text()), Ok(PNG));
    assert_eq!(detect("photo", &jpeg_with_gps(1)), Ok(JPEG));
    assert_eq!(detect("photo.jpg", &png_with_text()), Ok(PNG));
    assert_eq!(detect("notes.log", b"line one\nline two"), Ok(TEXT));
    assert_eq!(detect("evil.svg", b"just text"), Err(Refusal::Type));
    assert_eq!(detect("page.html", b"plain"), Err(Refusal::Type));
    assert_eq!(detect("page.htm", &png_with_text()), Err(Refusal::Type));
    assert_eq!(
        detect("evil.txt", b"<html><script>x</script></html>"),
        Err(Refusal::Type)
    );
    assert_eq!(detect("app.exe", b"MZ\x90\0"), Err(Refusal::Type));
    assert_eq!(detect("empty.txt", b""), Err(Refusal::Empty));
    assert!(allowed_name("Скриншот 2026.JPEG"));
    assert!(!allowed_name("vector.SVG"));
    assert_eq!(by_mime("image/png; charset=binary"), Some(PNG));
    assert_eq!(by_mime("image/svg+xml"), None);
    assert_eq!(by_extension("a.LOG"), Some(TEXT));
}

#[test]
fn имя_файла_чистится_и_получает_расширение_по_содержимому() {
    assert_eq!(
        file_name("C:\\Users\\me\\Скриншот.PNG", PNG),
        "Скриншот.png"
    );
    assert_eq!(file_name("../../etc/passwd", TEXT), "passwd.txt");
    assert_eq!(file_name("photo.jpg", PNG), "photo.png");
    assert_eq!(file_name("app.log", TEXT), "app.log");
    assert_eq!(file_name("report.jpeg", JPEG), "report.jpeg");
    assert_eq!(file_name("", JPEG), "photo.jpg");
    assert_eq!(file_name("...", PDF), "file.pdf");
    assert_eq!(file_name("a\"b<c>\n.pdf", PDF), "a b c.pdf");
    let long = format!("{}.txt", "я".repeat(300));
    assert_eq!(file_name(&long, TEXT).chars().count(), 104);
}

#[test]
fn из_jpeg_убираются_метаданные_а_поворот_остаётся() {
    let original = jpeg_with_gps(6);
    assert!(contains(&original, b"GPSLatitude"));
    let cleaned = clean(JPEG, original).unwrap();
    assert!(!contains(&cleaned.bytes, b"GPSLatitude"));
    assert!(!contains(&cleaned.bytes, b"xmpmeta"));
    assert!(!contains(&cleaned.bytes, b"Moscow"));
    assert!(!contains(&cleaned.bytes, b"comment with location"));
    assert!(!contains(&cleaned.bytes, b"trailing motion photo"));
    assert!(contains(&cleaned.bytes, b"ICC_PROFILE"));
    assert!(contains(&cleaned.bytes, b"JFIF"));
    assert!(cleaned.bytes.ends_with(&[0xFF, 0xD9]));
    assert!(contains(
        &cleaned.bytes,
        &[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56]
    ));
    let exif_at = cleaned
        .bytes
        .windows(6)
        .position(|w| w == b"Exif\0\0")
        .unwrap();
    let jfif_at = cleaned.bytes.windows(4).position(|w| w == b"JFIF").unwrap();
    assert!(jfif_at < exif_at);
    assert_eq!(
        &cleaned.bytes[exif_at + 6..exif_at + 6 + 22],
        b"MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0"
    );
    assert_eq!((cleaned.width, cleaned.height), (Some(480), Some(640)));
    let straight = clean(JPEG, jpeg_with_gps(1)).unwrap();
    assert!(!contains(&straight.bytes, b"Exif"));
    assert_eq!((straight.width, straight.height), (Some(640), Some(480)));
    assert_eq!(clean(JPEG, b"\xFF\xD8\x00broken".to_vec()), None);
}

#[test]
fn из_png_и_webp_убираются_метаданные() {
    let png = clean(PNG, png_with_text()).unwrap();
    assert!(!contains(&png.bytes, b"Moscow"));
    assert!(!contains(&png.bytes, b"eXIf"));
    assert!(contains(&png.bytes, b"IDAT"));
    assert!(png.bytes.ends_with(&png_chunk(b"IEND", b"")));
    assert_eq!((png.width, png.height), (Some(640), Some(480)));
    let webp = clean(WEBP, webp_with_exif()).unwrap();
    assert!(!contains(&webp.bytes, b"GPS"));
    assert!(!contains(&webp.bytes, b"XMP "));
    assert!(contains(&webp.bytes, b"VP8L"));
    let riff = u32::from_le_bytes(webp.bytes[4..8].try_into().unwrap()) as usize;
    assert_eq!(riff + 8, webp.bytes.len());
    assert_eq!(webp.bytes[20] & 0x0C, 0);
    assert_eq!((webp.width, webp.height), (Some(800), Some(480)));
    let gif = clean(GIF, b"GIF89a\x40\x01\xF0\x00rest".to_vec()).unwrap();
    assert_eq!((gif.width, gif.height), (Some(320), Some(240)));
    let pdf = clean(PDF, b"%PDF-1.4".to_vec()).unwrap();
    assert_eq!(pdf.bytes, b"%PDF-1.4");
    assert_eq!(PDF.kind, FileKind::File);
}

#[test]
fn настройки_модуля_проверяются_и_берут_умолчания() {
    let limits = Limits::of(&ModuleConfig::default());
    assert_eq!(limits, Limits::default());
    assert_eq!(limits.max_bytes, 10 * MB);
    assert_eq!(limits.max_files, 5);
    assert_eq!(limits.keep_days, 0);
    let custom = ModuleConfig::from_value(&serde_json::json!({
        "max_mb": "20", "max_files": 1, "keep_days": 30
    }));
    let parsed = Limits::parse(&custom).unwrap();
    assert_eq!(
        (parsed.max_mb(), parsed.max_files, parsed.keep_days),
        (20, 1, 30)
    );
    for bad in [
        serde_json::json!({ "max_mb": 21 }),
        serde_json::json!({ "max_mb": 0 }),
        serde_json::json!({ "max_files": 11 }),
        serde_json::json!({ "keep_days": -1 }),
        serde_json::json!({ "max_mb": "много" }),
    ] {
        assert!(
            Limits::parse(&ModuleConfig::from_value(&bad)).is_err(),
            "{bad}"
        );
    }
    let module = TicketFilesModule;
    assert!(module.validate_config(&custom).is_ok());
    assert_eq!(module.settings_schema().len(), 3);
    assert_eq!(module.cleanup_tasks()[0].name, PURGE_TASK);
}
