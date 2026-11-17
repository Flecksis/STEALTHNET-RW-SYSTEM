use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;

pub const STEP_SECONDS: i64 = 30;
pub const DIGITS: u32 = 6;
pub const SECRET_BYTES: usize = 20;
pub const MIN_SECRET_BYTES: usize = 16;

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn generate_secret() -> String {
    let mut raw = [0u8; SECRET_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    base32_encode(&raw)
}

pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in data {
        acc = (acc << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[((acc >> bits) & 31) as usize]));
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((acc << (5 - bits)) & 31) as usize]));
    }
    out
}

pub fn base32_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 5 / 8);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for ch in input.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let upper = ch.to_ascii_uppercase();
        let value = ALPHABET.iter().position(|&a| char::from(a) == upper)? as u32;
        acc = (acc << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
        acc &= (1 << bits) - 1;
    }
    Some(out)
}

pub fn code_at(key: &[u8], counter: u64, digits: u32) -> Option<String> {
    let mut mac = Hmac::<Sha1>::new_from_slice(key).ok()?;
    mac.update(&counter.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = usize::from(hash.last()? & 0x0F);
    let slice = hash.get(offset..offset + 4)?;
    let binary = (u32::from(slice[0] & 0x7F) << 24)
        | (u32::from(slice[1]) << 16)
        | (u32::from(slice[2]) << 8)
        | u32::from(slice[3]);
    let modulo = 10u64.checked_pow(digits)?;
    let value = u64::from(binary) % modulo;
    Some(format!("{value:0width$}", width = digits as usize))
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

pub fn normalize_code(code: &str) -> Option<String> {
    let cleaned: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    (cleaned.len() == DIGITS as usize && cleaned.bytes().all(|b| b.is_ascii_digit()))
        .then_some(cleaned)
}

pub fn verify(secret: &str, code: &str, unix_seconds: i64) -> Option<i64> {
    let code = normalize_code(code)?;
    let key = base32_decode(secret).filter(|k| !k.is_empty())?;
    let current = unix_seconds.div_euclid(STEP_SECONDS);
    let mut matched = None;
    for step in [current - 1, current, current + 1] {
        let Ok(counter) = u64::try_from(step) else {
            continue;
        };
        let Some(expected) = code_at(&key, counter, DIGITS) else {
            continue;
        };
        if same(expected.as_bytes(), code.as_bytes()) && matched.is_none() {
            matched = Some(step);
        }
    }
    matched
}

fn uri_escape(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

pub fn provisioning_uri(secret: &str, account: &str, issuer: &str) -> String {
    let issuer = uri_escape(issuer);
    format!(
        "otpauth://totp/{issuer}:{}?secret={secret}&issuer={issuer}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
        uri_escape(account)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_KEY: &[u8] = b"12345678901234567890";

    #[test]
    fn коды_совпадают_с_векторами_rfc_6238() {
        let vectors: [(i64, &str); 6] = [
            (59, "94287082"),
            (1_111_111_109, "07081804"),
            (1_111_111_111, "14050471"),
            (1_234_567_890, "89005924"),
            (2_000_000_000, "69279037"),
            (20_000_000_000, "65353130"),
        ];
        for (time, expected) in vectors {
            let counter = u64::try_from(time / STEP_SECONDS).unwrap();
            assert_eq!(
                code_at(RFC_KEY, counter, 8).as_deref(),
                Some(expected),
                "{time}"
            );
        }
    }

    #[test]
    fn шестизначный_код_rfc_принимается_с_номером_шага() {
        let secret = base32_encode(RFC_KEY);
        assert_eq!(verify(&secret, "287082", 59), Some(1));
        assert_eq!(verify(&secret, "081 804", 1_111_111_109), Some(37_037_036));
    }

    #[test]
    fn соседние_окна_принимаются_а_дальние_нет() {
        let secret = generate_secret();
        let key = base32_decode(&secret).unwrap();
        let now = 1_800_000_000_i64;
        let step = now / STEP_SECONDS;
        let previous = code_at(&key, (step - 1) as u64, DIGITS).unwrap();
        let next = code_at(&key, (step + 1) as u64, DIGITS).unwrap();
        let far = code_at(&key, (step - 2) as u64, DIGITS).unwrap();
        assert_eq!(verify(&secret, &previous, now), Some(step - 1));
        assert_eq!(verify(&secret, &next, now), Some(step + 1));
        if far != previous && far != next && far != code_at(&key, step as u64, DIGITS).unwrap() {
            assert_eq!(verify(&secret, &far, now), None);
        }
    }

    #[test]
    fn мусор_вместо_кода_не_проходит() {
        let secret = generate_secret();
        for code in ["", "12345", "1234567", "abcdef", "12345x"] {
            assert_eq!(verify(&secret, code, 1_800_000_000), None, "{code}");
        }
        assert_eq!(verify("не base32", "123456", 1_800_000_000), None);
    }

    #[test]
    fn base32_как_в_rfc_4648() {
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32_encode(b"f"), "MY");
        assert_eq!(base32_decode("mzxw6ytboi").unwrap(), b"foobar");
        assert_eq!(base32_decode("MZXW 6YTB OI======").unwrap(), b"foobar");
        assert_eq!(base32_decode(&base32_encode(RFC_KEY)).unwrap(), RFC_KEY);
        assert!(base32_decode("1").is_none());
        assert_eq!(generate_secret().len(), 32);
    }

    #[test]
    fn ссылка_для_приложения_аутентификатора() {
        let uri = provisioning_uri("JBSWY3DPEHPK3PXP", "owner@example.com", "STEALTHNET VPN");
        assert_eq!(
            uri,
            "otpauth://totp/STEALTHNET%20VPN:owner%40example.com?secret=JBSWY3DPEHPK3PXP&issuer=STEALTHNET%20VPN&algorithm=SHA1&digits=6&period=30"
        );
    }
}
