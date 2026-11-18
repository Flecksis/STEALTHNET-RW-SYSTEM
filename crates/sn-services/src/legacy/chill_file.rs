use super::archive::OpenError;
use super::table::{Dump, Table, parse_time};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use serde::Deserialize;
use std::io::{BufRead, Read};
use std::path::Path;

pub const SEALED: &[u8] = b"CRBAK1";
const KIND: &str = "chill-remna-bot-backup";
const SALT: usize = 16;
const NONCE: usize = 12;
const TAG: usize = 16;
const MAX_LINE: u64 = 32 << 20;

#[derive(Deserialize)]
struct Mark {
    t: String,
    kind: Option<String>,
    format: Option<i64>,
    created_at: Option<String>,
    name: Option<String>,
    cols: Option<Vec<String>>,
    rows: Option<u64>,
}

fn broken() -> OpenError {
    OpenError::Unsupported(
        "Файл бэкапа Chill Remna bot повреждён. Скачайте его из Telegram заново".into(),
    )
}

fn cut() -> OpenError {
    OpenError::Unsupported(
        "Файл бэкапа Chill Remna bot обрывается на середине. Скачайте его из Telegram заново"
            .into(),
    )
}

pub fn is_sealed(head: &[u8]) -> bool {
    head.starts_with(SEALED)
}

fn key(password: &str, salt: &[u8]) -> Result<[u8; 32], OpenError> {
    let params = scrypt::Params::new(15, 8, 1, 32)
        .map_err(|err| OpenError::Io(format!("ключ копии не вычислился: {err}")))?;
    let mut key = [0u8; 32];
    scrypt::scrypt(password.as_bytes(), salt, &params, &mut key)
        .map_err(|err| OpenError::Io(format!("ключ копии не вычислился: {err}")))?;
    Ok(key)
}

pub fn open_sealed(file: &[u8], password: &str) -> Result<Vec<u8>, OpenError> {
    let rest = file.strip_prefix(SEALED).ok_or_else(broken)?;
    if rest.len() < SALT + NONCE + TAG {
        return Err(cut());
    }
    let (salt, rest) = rest.split_at(SALT);
    let (nonce, data) = rest.split_at(NONCE);
    let mut tried = Vec::new();
    for candidate in [password, password.trim()] {
        if candidate.is_empty() || tried.contains(&candidate) {
            continue;
        }
        tried.push(candidate);
        let cipher = Aes256Gcm::new_from_slice(&key(candidate, salt)?)
            .map_err(|err| OpenError::Io(err.to_string()))?;
        if let Ok(plain) = cipher.decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: data,
                aad: SEALED,
            },
        ) {
            return Ok(plain);
        }
    }
    Err(OpenError::WrongPassword)
}

pub fn unseal(path: &Path, password: Option<&str>, target: &Path) -> Result<bool, OpenError> {
    let mut head = [0u8; 6];
    let mut file = std::fs::File::open(path)?;
    let mut filled = 0;
    while filled < head.len() {
        let n = file.read(&mut head[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    if !is_sealed(&head[..filled]) {
        return Ok(false);
    }
    let Some(password) = password.filter(|p| !p.trim().is_empty()) else {
        return Err(OpenError::NeedsPassword);
    };
    drop(file);
    let sealed = std::fs::read(path)?;
    let plain = open_sealed(&sealed, password)?;
    drop(sealed);
    std::fs::write(target, plain)?;
    Ok(true)
}

pub fn is_lines(head: &[u8]) -> bool {
    let head = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    let first = head.split(|b| *b == b'\n').next().unwrap_or(head);
    serde_json::from_slice::<Mark>(first)
        .is_ok_and(|m| m.t == "h" && m.kind.as_deref() == Some(KIND))
}

fn cell(value: serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(flag) => Some(if flag { "t" } else { "f" }.into()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(s),
        serde_json::Value::Object(map) => match map.get("x").and_then(|x| x.as_str()) {
            Some(encoded) => base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            None => Some(serde_json::Value::Object(map).to_string()),
        },
        other => Some(other.to_string()),
    }
}

struct Open {
    name: String,
    table: Option<Table>,
    width: usize,
    count: u64,
}

pub fn read_lines(
    mut reader: impl BufRead,
    wanted: &dyn Fn(&str) -> bool,
    dump: &mut Dump,
) -> Result<(), OpenError> {
    let mut line = Vec::with_capacity(64 * 1024);
    let mut next = |line: &mut Vec<u8>| -> Result<bool, OpenError> {
        line.clear();
        let read = (&mut reader).take(MAX_LINE).read_until(b'\n', line)?;
        if read == 0 {
            return Ok(false);
        }
        if line.last() != Some(&b'\n') && read as u64 >= MAX_LINE {
            return Err(broken());
        }
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        Ok(true)
    };
    if !next(&mut line)? {
        return Err(cut());
    }
    let head: Mark = serde_json::from_slice(&line).map_err(|_| broken())?;
    if head.t != "h" || head.kind.as_deref() != Some(KIND) {
        return Err(broken());
    }
    if head.format != Some(1) {
        return Err(OpenError::Unsupported(
            "Бэкап сделала более новая версия Chill Remna bot, такой формат перенос ещё не читает"
                .into(),
        ));
    }
    dump.format = "chill";
    if dump.dumped_at.is_none() {
        dump.dumped_at = head.created_at.as_deref().and_then(parse_time);
    }
    let mut open: Option<Open> = None;
    loop {
        if !next(&mut line)? {
            return Err(cut());
        }
        match line.first() {
            Some(b'[') => {
                let Some(current) = open.as_mut() else {
                    return Err(broken());
                };
                let row: Vec<serde_json::Value> =
                    serde_json::from_slice(&line).map_err(|_| broken())?;
                if row.len() != current.width {
                    return Err(broken());
                }
                current.count += 1;
                if let Some(table) = current.table.as_mut() {
                    let cells: Vec<Option<String>> = row.into_iter().map(cell).collect();
                    table.push(cells.iter().map(Option::as_deref));
                }
            }
            Some(b'{') => {
                let mark: Mark = serde_json::from_slice(&line).map_err(|_| broken())?;
                match mark.t.as_str() {
                    "tb" => {
                        let (Some(name), Some(cols)) = (mark.name, mark.cols) else {
                            return Err(broken());
                        };
                        if open.is_some() || cols.is_empty() {
                            return Err(broken());
                        }
                        let table = wanted(&name).then(|| Table::new(cols.clone()));
                        open = Some(Open {
                            name,
                            table,
                            width: cols.len(),
                            count: 0,
                        });
                    }
                    "te" => {
                        let Some(done) = open.take() else {
                            return Err(broken());
                        };
                        if mark.rows != Some(done.count) {
                            return Err(broken());
                        }
                        if let Some(table) = done.table {
                            dump.tables.insert(done.name, table);
                        }
                    }
                    "end" if open.is_none() => return Ok(()),
                    _ => return Err(broken()),
                }
            }
            _ => return Err(broken()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINES: &str = "{\"t\":\"h\",\"kind\":\"chill-remna-bot-backup\",\"format\":1,\"created_at\":\"2026-11-18T10:00:00Z\",\"db\":\"sqlite\",\"schema\":77}\n\
{\"t\":\"tb\",\"name\":\"users\",\"cols\":[\"telegram_id\",\"username\",\"blocked\"]}\n\
[5001,\"katya\",0]\n\
[5002,{\"x\":\"/w==\"},1]\n\
{\"t\":\"te\",\"rows\":2}\n\
{\"t\":\"tb\",\"name\":\"media_cache\",\"cols\":[\"section\"]}\n\
[\"menu\"]\n\
{\"t\":\"te\",\"rows\":1}\n\
{\"t\":\"end\",\"tables\":2}\n";

    #[test]
    fn строки_копии_собираются_в_таблицы() {
        assert!(is_lines(LINES.as_bytes()));
        assert!(!is_lines(b"{\"metadata\":{}}"));
        let mut dump = Dump::default();
        read_lines(LINES.as_bytes(), &|t| t == "users", &mut dump).unwrap();
        assert_eq!(dump.format, "chill");
        assert!(dump.dumped_at.is_some());
        assert!(dump.table("media_cache").is_none());
        let rows: Vec<_> = dump.rows("users").collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].i64("telegram_id"), Some(5001));
        assert_eq!(rows[1].raw("username"), Some("\u{fffd}"));
        assert_eq!(rows[1].bool("blocked"), Some(true));
    }

    #[test]
    fn обрезанная_или_испорченная_копия_даёт_понятную_ошибку() {
        let cut_at = LINES.find("{\"t\":\"end\"").unwrap();
        let mut dump = Dump::default();
        let err = read_lines(&LINES.as_bytes()[..cut_at], &|_| true, &mut dump).unwrap_err();
        assert!(matches!(err, OpenError::Unsupported(text) if text.contains("обрывается")));
        let short = LINES.replace("\"rows\":2", "\"rows\":3");
        let err = read_lines(short.as_bytes(), &|_| true, &mut Dump::default()).unwrap_err();
        assert!(matches!(err, OpenError::Unsupported(text) if text.contains("повреждён")));
        let newer = LINES.replace("\"format\":1", "\"format\":2");
        let err = read_lines(newer.as_bytes(), &|_| true, &mut Dump::default()).unwrap_err();
        assert!(matches!(err, OpenError::Unsupported(text) if text.contains("новая версия")));
    }

    #[test]
    fn запечатанный_файл_открывается_только_верным_паролем() {
        let file = include_bytes!("fixtures/chill.bak");
        assert!(is_sealed(file));
        assert_eq!(
            open_sealed(file, "не тот").unwrap_err(),
            OpenError::WrongPassword
        );
        let plain = open_sealed(file, " fixture-pass\n").unwrap();
        assert_eq!(&plain[..2], &[0x1f, 0x8b]);
        let mut truncated = file.to_vec();
        truncated.truncate(30);
        assert!(matches!(
            open_sealed(&truncated, "fixture-pass"),
            Err(OpenError::Unsupported(_))
        ));
    }
}
