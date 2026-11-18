use super::kind::{FileType, GIF, JPEG, PNG, WEBP};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleaned {
    pub bytes: Vec<u8>,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

pub fn clean(file: FileType, bytes: Vec<u8>) -> Option<Cleaned> {
    let (bytes, size) = if file == JPEG {
        let (bytes, orientation, size) = jpeg(&bytes)?;
        let size = size.map(|(w, h)| match orientation {
            Some(5..=8) => (h, w),
            _ => (w, h),
        });
        (bytes, size)
    } else if file == PNG {
        let size = png_size(&bytes);
        (png(&bytes)?, size)
    } else if file == WEBP {
        let size = webp_size(&bytes);
        (webp(&bytes)?, size)
    } else if file == GIF {
        let size = gif_size(&bytes);
        (bytes, size)
    } else {
        (bytes, None)
    };
    let (width, height) = match size {
        Some((w, h)) if w > 0 && h > 0 => (i32::try_from(w).ok(), i32::try_from(h).ok()),
        _ => (None, None),
    };
    Some(Cleaned {
        bytes,
        width,
        height,
    })
}

fn u16_be(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn u32_be(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u32_le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn exif_orientation(tiff: &[u8]) -> Option<u16> {
    let little = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let pair = [*tiff.get(at)?, *tiff.get(at + 1)?];
        Some(if little {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let quad: [u8; 4] = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(quad)
        } else {
            u32::from_be_bytes(quad)
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    let count = usize::from(u16_at(ifd)?);
    (0..count.min(256)).find_map(|i| {
        let entry = ifd + 2 + i * 12;
        (u16_at(entry)? == 0x0112 && u16_at(entry + 2)? == 3)
            .then(|| u16_at(entry + 8))
            .flatten()
            .filter(|o| (1..=8).contains(o))
    })
}

fn orientation_segment(orientation: u16) -> Vec<u8> {
    let mut out = vec![0xFF, 0xE1, 0x00, 0x22];
    out.extend_from_slice(b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01");
    out.extend_from_slice(&orientation.to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    out
}

fn keep_jpeg_segment(marker: u8, payload: &[u8]) -> bool {
    match marker {
        0xE2 => payload.starts_with(b"ICC_PROFILE\0"),
        0xE1 | 0xE3..=0xED | 0xEF | 0xFE => false,
        _ => true,
    }
}

type JpegParts = (Vec<u8>, Option<u16>, Option<(u32, u32)>);

fn jpeg(b: &[u8]) -> Option<JpegParts> {
    if !b.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&[0xFF, 0xD8]);
    let mut orientation = None;
    let mut size = None;
    let mut insert_at = 2usize;
    let mut i = 2usize;
    loop {
        if *b.get(i)? != 0xFF {
            return None;
        }
        while b.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        let marker = *b.get(i + 1)?;
        match marker {
            0xD9 => {
                out.extend_from_slice(&[0xFF, 0xD9]);
                break;
            }
            0x01 | 0xD0..=0xD7 => {
                out.extend_from_slice(&[0xFF, marker]);
                i += 2;
                continue;
            }
            _ => {}
        }
        let len = usize::from(u16_be(b, i + 2)?);
        if len < 2 {
            return None;
        }
        let end = i + 2 + len;
        let segment = b.get(i..end)?;
        let payload = &segment[4..];
        if marker == 0xE1 && payload.starts_with(b"Exif\0\0") {
            orientation = orientation.or_else(|| exif_orientation(&payload[6..]));
        }
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) && size.is_none()
        {
            let height = u32::from(u16_be(payload, 1)?);
            let width = u32::from(u16_be(payload, 3)?);
            size = Some((width, height));
        }
        if keep_jpeg_segment(marker, payload) {
            out.extend_from_slice(segment);
            if marker == 0xE0 && insert_at == 2 && out.len() == 2 + segment.len() {
                insert_at = out.len();
            }
        }
        i = end;
        if marker == 0xDA {
            let start = i;
            while i + 1 < b.len() {
                if b[i] == 0xFF && !matches!(b[i + 1], 0x00 | 0xD0..=0xD7 | 0xFF) {
                    break;
                }
                i += 1;
            }
            if i + 1 >= b.len() {
                out.extend_from_slice(&b[start..]);
                break;
            }
            out.extend_from_slice(&b[start..i]);
        }
    }
    if let Some(o) = orientation.filter(|o| *o != 1) {
        let segment = orientation_segment(o);
        out.splice(insert_at..insert_at, segment);
    }
    Some((out, orientation, size))
}

const PNG_DROP: [&[u8; 4]; 5] = [b"eXIf", b"tEXt", b"zTXt", b"iTXt", b"tIME"];

fn png(b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(b.get(..8)?);
    let mut i = 8usize;
    loop {
        let len = usize::try_from(u32_be(b, i)?).ok()?;
        let end = i.checked_add(12)?.checked_add(len)?;
        let chunk = b.get(i..end)?;
        let kind = &chunk[4..8];
        if !PNG_DROP.iter().any(|drop| kind == drop.as_slice()) {
            out.extend_from_slice(chunk);
        }
        i = end;
        if kind == b"IEND" {
            return Some(out);
        }
    }
}

fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    (b.get(12..16)? == b"IHDR").then_some(())?;
    Some((u32_be(b, 16)?, u32_be(b, 20)?))
}

fn webp(b: &[u8]) -> Option<Vec<u8>> {
    let declared = usize::try_from(u32_le(b, 4)?).ok()?;
    let end = declared.checked_add(8)?.min(b.len());
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(b"RIFF\0\0\0\0WEBP");
    let mut i = 12usize;
    while i + 8 <= end {
        let fourcc = &b[i..i + 4];
        let size = usize::try_from(u32_le(b, i + 4)?).ok()?;
        let padded = size.checked_add(size & 1)?;
        let stop = i.checked_add(8)?.checked_add(padded)?.min(end);
        let chunk = b.get(i..stop)?;
        if fourcc != b"EXIF" && fourcc != b"XMP " {
            let at = out.len();
            out.extend_from_slice(chunk);
            if fourcc == b"VP8X" && chunk.len() > 8 {
                out[at + 8] &= !0x0C;
            }
        }
        i = stop;
    }
    let riff = u32::try_from(out.len() - 8).ok()?;
    out[4..8].copy_from_slice(&riff.to_le_bytes());
    Some(out)
}

fn u24_le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
        *b.get(at + 2)?,
        0,
    ]))
}

fn webp_size(b: &[u8]) -> Option<(u32, u32)> {
    let fourcc = b.get(12..16)?;
    let data = 20usize;
    match fourcc {
        b"VP8X" => Some((u24_le(b, data + 4)? + 1, u24_le(b, data + 7)? + 1)),
        b"VP8 " => {
            (b.get(data + 3..data + 6)? == &[0x9d, 0x01, 0x2a][..]).then_some(())?;
            let w = u32::from(u16::from_le_bytes([*b.get(data + 6)?, *b.get(data + 7)?]) & 0x3FFF);
            let h = u32::from(u16::from_le_bytes([*b.get(data + 8)?, *b.get(data + 9)?]) & 0x3FFF);
            Some((w, h))
        }
        b"VP8L" => {
            (*b.get(data)? == 0x2f).then_some(())?;
            let bits = u32_le(b, data + 1)?;
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        _ => None,
    }
}

fn gif_size(b: &[u8]) -> Option<(u32, u32)> {
    let w = u16::from_le_bytes([*b.get(6)?, *b.get(7)?]);
    let h = u16::from_le_bytes([*b.get(8)?, *b.get(9)?]);
    Some((u32::from(w), u32::from(h)))
}
