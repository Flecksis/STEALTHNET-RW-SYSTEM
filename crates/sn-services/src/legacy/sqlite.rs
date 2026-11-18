use super::archive::OpenError;
use super::table::{Dump, Table};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const MAGIC: &[u8] = b"SQLite format 3\0";

const HEADER: usize = 100;
const LEAF_TABLE: u8 = 0x0d;
const INTERIOR_TABLE: u8 = 0x05;

fn broken() -> OpenError {
    OpenError::Unsupported(
        "Файл базы SQLite повреждён или скопирован не до конца. Остановите старого бота и скопируйте файл базы заново".into(),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

enum Value {
    Null,
    Int(i64),
    Real(f64),
    Bytes(Vec<u8>),
}

struct Column {
    name: String,
    rowid: bool,
    default: Option<String>,
}

struct Pages {
    file: File,
    size: usize,
    usable: usize,
    count: u64,
    encoding: Encoding,
}

impl Pages {
    fn open(path: &Path) -> Result<Self, OpenError> {
        let mut file = File::open(path)?;
        let mut header = [0u8; HEADER];
        file.read_exact(&mut header).map_err(|_| broken())?;
        if !header.starts_with(MAGIC) {
            return Err(OpenError::Unsupported("Это не файл базы SQLite".into()));
        }
        let raw = u16::from_be_bytes([header[16], header[17]]);
        let size = if raw == 1 { 65_536 } else { usize::from(raw) };
        if !(512..=65_536).contains(&size) || !size.is_power_of_two() {
            return Err(broken());
        }
        let reserved = usize::from(header[20]);
        if !matches!(header[18], 1 | 2) || !matches!(header[19], 1 | 2) || reserved + 480 > size {
            return Err(broken());
        }
        let encoding = match u32::from_be_bytes([header[56], header[57], header[58], header[59]]) {
            0 | 1 => Encoding::Utf8,
            2 => Encoding::Utf16Le,
            3 => Encoding::Utf16Be,
            _ => return Err(broken()),
        };
        let length = file.metadata()?.len();
        Ok(Pages {
            file,
            size,
            usable: size - reserved,
            count: length / size as u64,
            encoding,
        })
    }

    fn read(&mut self, number: u32) -> Result<Vec<u8>, OpenError> {
        if number == 0 || u64::from(number) > self.count {
            return Err(broken());
        }
        let mut page = vec![0u8; self.size];
        self.file
            .seek(SeekFrom::Start((u64::from(number) - 1) * self.size as u64))?;
        self.file.read_exact(&mut page).map_err(|_| broken())?;
        Ok(page)
    }

    fn text(&self, bytes: &[u8]) -> String {
        match self.encoding {
            Encoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
            Encoding::Utf16Le | Encoding::Utf16Be => {
                let units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|pair| match self.encoding {
                        Encoding::Utf16Le => u16::from_le_bytes([pair[0], pair[1]]),
                        _ => u16::from_be_bytes([pair[0], pair[1]]),
                    })
                    .collect();
                String::from_utf16_lossy(&units)
            }
        }
    }

    fn payload(&mut self, page: &[u8], at: usize, total: u64) -> Result<Vec<u8>, OpenError> {
        let usable = self.usable as u64;
        let max_local = usable - 35;
        let local: u64 = if total <= max_local {
            total
        } else {
            let min_local = ((usable - 12) * 32 / 255) - 23;
            let spill = min_local + (total - min_local) % (usable - 4);
            if spill <= max_local { spill } else { min_local }
        };
        let local = usize::try_from(local).map_err(|_| broken())?;
        let local_end = at.checked_add(local).ok_or_else(broken)?;
        let mut out = page.get(at..local_end).ok_or_else(broken)?.to_vec();
        if out.len() as u64 == total {
            return Ok(out);
        }
        let mut next = read_u32(page, local_end)?;
        let mut hops = 0u64;
        while (out.len() as u64) < total {
            hops += 1;
            if next == 0 || hops > self.count {
                return Err(broken());
            }
            let overflow = self.read(next)?;
            next = read_u32(&overflow, 0)?;
            let want = (total - out.len() as u64).min(usable - 4) as usize;
            out.extend_from_slice(overflow.get(4..4 + want).ok_or_else(broken)?);
        }
        Ok(out)
    }

    fn rows(
        &mut self,
        root: u32,
        mut visit: impl FnMut(&Self, i64, Vec<Value>) -> Result<(), OpenError>,
    ) -> Result<(), OpenError> {
        let mut stack = vec![root];
        let mut seen: HashSet<u32> = HashSet::new();
        while let Some(number) = stack.pop() {
            if !seen.insert(number) {
                return Err(broken());
            }
            let page = self.read(number)?;
            let start = if number == 1 { HEADER } else { 0 };
            let kind = *page.get(start).ok_or_else(broken)?;
            let cells = usize::from(read_u16(&page, start + 3)?);
            match kind {
                LEAF_TABLE => {
                    for i in 0..cells {
                        let mut at = usize::from(read_u16(&page, start + 8 + 2 * i)?);
                        let total = varint(&page, &mut at)?;
                        let rowid = varint(&page, &mut at)? as i64;
                        let record = self.payload(&page, at, total)?;
                        visit(self, rowid, record_values(&record)?)?;
                    }
                }
                INTERIOR_TABLE => {
                    stack.push(read_u32(&page, start + 8)?);
                    for i in (0..cells).rev() {
                        let at = usize::from(read_u16(&page, start + 12 + 2 * i)?);
                        stack.push(read_u32(&page, at)?);
                    }
                }
                _ => return Err(broken()),
            }
        }
        Ok(())
    }
}

fn read_u16(page: &[u8], at: usize) -> Result<u16, OpenError> {
    let bytes = page.get(at..at + 2).ok_or_else(broken)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32(page: &[u8], at: usize) -> Result<u32, OpenError> {
    let bytes = page.get(at..at + 4).ok_or_else(broken)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn varint(data: &[u8], at: &mut usize) -> Result<u64, OpenError> {
    let mut value = 0u64;
    for i in 0..9 {
        let byte = *data.get(*at).ok_or_else(broken)?;
        *at += 1;
        if i == 8 {
            return Ok((value << 8) | u64::from(byte));
        }
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Ok(value)
}

fn signed(bytes: &[u8]) -> i64 {
    let mut value: i64 = if bytes.first().is_some_and(|b| b & 0x80 != 0) {
        -1
    } else {
        0
    };
    for byte in bytes {
        value = (value << 8) | i64::from(*byte);
    }
    value
}

fn record_values(record: &[u8]) -> Result<Vec<Value>, OpenError> {
    let mut at = 0usize;
    let header_len = varint(record, &mut at)? as usize;
    if header_len > record.len() {
        return Err(broken());
    }
    let mut types = Vec::new();
    while at < header_len {
        types.push(varint(record, &mut at)?);
    }
    let mut body = header_len;
    let mut values = Vec::with_capacity(types.len());
    for kind in types {
        let len = match kind {
            0 | 8 | 9 => 0,
            1 => 1,
            2 => 2,
            3 => 3,
            4 => 4,
            5 => 6,
            6 | 7 => 8,
            10 | 11 => return Err(broken()),
            n => ((n - 12) / 2) as usize,
        };
        let end = body.checked_add(len).ok_or_else(broken)?;
        let bytes = record.get(body..end).ok_or_else(broken)?;
        body = end;
        values.push(match kind {
            0 => Value::Null,
            8 => Value::Int(0),
            9 => Value::Int(1),
            1..=6 => Value::Int(signed(bytes)),
            7 => {
                let mut raw = [0u8; 8];
                raw.copy_from_slice(bytes);
                Value::Real(f64::from_bits(u64::from_be_bytes(raw)))
            }
            _ => Value::Bytes(bytes.to_vec()),
        });
    }
    Ok(values)
}

fn split_top(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match quote {
            Some(q) => {
                current.push(c);
                if c == q || (q == '[' && c == ']') {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' | '`' | '[' => {
                    quote = Some(c);
                    current.push(c);
                }
                '(' => {
                    depth += 1;
                    current.push(c);
                }
                ')' => {
                    depth -= 1;
                    current.push(c);
                }
                ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
                _ => current.push(c),
            },
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

fn unquote(word: &str) -> String {
    let word = word.trim();
    let inner = word
        .strip_prefix('"')
        .and_then(|w| w.strip_suffix('"'))
        .or_else(|| word.strip_prefix('`').and_then(|w| w.strip_suffix('`')))
        .or_else(|| word.strip_prefix('[').and_then(|w| w.strip_suffix(']')))
        .or_else(|| word.strip_prefix('\'').and_then(|w| w.strip_suffix('\'')));
    match inner {
        Some(inner) => inner.to_string(),
        None => word.to_ascii_lowercase(),
    }
}

fn first_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    let close = match text.chars().next() {
        Some('"') => Some('"'),
        Some('`') => Some('`'),
        Some('[') => Some(']'),
        Some('\'') => Some('\''),
        _ => None,
    };
    let end = match close {
        Some(close) => text[1..].find(close).map_or(text.len(), |i| i + 2),
        None => text
            .find(|c: char| c.is_whitespace() || c == '(')
            .unwrap_or(text.len()),
    };
    (&text[..end], &text[end..])
}

fn default_of(definition: &str) -> Option<String> {
    let upper = definition.to_ascii_uppercase();
    let mut from = 0;
    let at = loop {
        let found = upper[from..].find("DEFAULT")? + from;
        let end = found + "DEFAULT".len();
        let before = upper[..found].chars().next_back();
        let after = upper[end..].chars().next();
        if before.is_none_or(char::is_whitespace)
            && after.is_some_and(|c| {
                c.is_whitespace() || matches!(c, '\'' | '(' | '-' | '+') || c.is_ascii_digit()
            })
        {
            break end;
        }
        from = end;
    };
    let rest = definition[at..].trim_start();
    if let Some(body) = rest.strip_prefix('\'') {
        let mut out = String::new();
        let mut chars = body.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    out.push('\'');
                    continue;
                }
                return Some(out);
            }
            out.push(c);
        }
        return None;
    }
    let word: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        .collect();
    match word.to_ascii_uppercase().as_str() {
        "" | "NULL" | "CURRENT_TIMESTAMP" | "CURRENT_DATE" | "CURRENT_TIME" => None,
        "TRUE" => Some("1".into()),
        "FALSE" => Some("0".into()),
        _ if word.parse::<f64>().is_ok() => Some(word.trim_start_matches('+').to_string()),
        _ => None,
    }
}

const CONSTRAINTS: [&str; 5] = ["CONSTRAINT", "PRIMARY", "UNIQUE", "CHECK", "FOREIGN"];

fn columns(sql: &str) -> Option<Vec<Column>> {
    let open = sql.find('(')?;
    let close = sql.rfind(')')?;
    if close <= open || sql[close..].to_ascii_uppercase().contains("WITHOUT ROWID") {
        return None;
    }
    let mut out: Vec<Column> = Vec::new();
    let mut declared: Vec<String> = Vec::new();
    let mut table_key: Option<Vec<String>> = None;
    for part in split_top(&sql[open + 1..close]) {
        let part = part.trim();
        let (word, rest) = first_word(part);
        let upper = word.to_ascii_uppercase();
        if CONSTRAINTS.contains(&upper.as_str()) {
            let text = part.to_ascii_uppercase();
            if let (Some(at), Some(end)) = (text.find("PRIMARY KEY"), part.rfind(')'))
                && let Some(open) = part[at..].find('(')
            {
                table_key = Some(
                    part[at + open + 1..end]
                        .split(',')
                        .map(|c| unquote(first_word(c).0))
                        .collect(),
                );
            }
            continue;
        }
        let kind = rest
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches(',')
            .to_ascii_uppercase();
        let definition = format!(
            " {} ",
            rest.to_ascii_uppercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        );
        out.push(Column {
            name: unquote(word),
            rowid: kind == "INTEGER" && definition.contains(" PRIMARY KEY"),
            default: default_of(rest),
        });
        declared.push(kind);
    }
    if let Some(key) = table_key
        && key.len() == 1
        && let Some(at) = out.iter().position(|c| c.name == key[0])
        && declared[at] == "INTEGER"
    {
        out[at].rowid = true;
    }
    (!out.is_empty()).then_some(out)
}

fn cell(pages: &Pages, value: Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Int(v) => Some(v.to_string()),
        Value::Real(v) => Some(v.to_string()),
        Value::Bytes(bytes) => Some(pages.text(&bytes)),
    }
}

pub fn read(path: &Path, wanted: &dyn Fn(&str) -> bool, dump: &mut Dump) -> Result<(), OpenError> {
    let mut pages = Pages::open(path)?;
    let mut schema: Vec<(String, u32, String)> = Vec::new();
    pages.rows(1, |pages, _, values| {
        let mut values = values.into_iter();
        let kind = values.next().and_then(|v| cell(pages, v));
        let name = values.next().and_then(|v| cell(pages, v));
        let _table = values.next();
        let root = match values.next() {
            Some(Value::Int(root)) => u32::try_from(root).ok(),
            _ => None,
        };
        let sql = values.next().and_then(|v| cell(pages, v));
        if let (Some("table"), Some(name), Some(root), Some(sql)) =
            (kind.as_deref(), name, root, sql)
            && root > 0
        {
            schema.push((name, root, sql));
        }
        Ok(())
    })?;
    if schema.is_empty() {
        return Err(OpenError::Unsupported(
            "В файле SQLite нет ни одной таблицы".into(),
        ));
    }
    for (name, root, sql) in schema {
        if !wanted(&name) {
            continue;
        }
        let Some(columns) = columns(&sql) else {
            continue;
        };
        let mut table = Table::new(columns.iter().map(|c| c.name.clone()).collect());
        pages.rows(root, |pages, rowid, values| {
            let mut values = values.into_iter();
            let row: Vec<Option<String>> = columns
                .iter()
                .map(|column| {
                    let value = values.next();
                    if column.rowid {
                        return Some(rowid.to_string());
                    }
                    match value {
                        Some(value) => cell(pages, value),
                        None => column.default.clone(),
                    }
                })
                .collect();
            table.push(row.iter().map(Option::as_deref));
            Ok(())
        })?;
        dump.tables.insert(name, table);
    }
    if dump.format.is_empty() {
        dump.format = "sqlite";
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn описание_таблицы_разбирается_с_изменениями_и_ключами() {
        let sql = "CREATE TABLE users (\n    telegram_id  INTEGER PRIMARY KEY,\n    \"created at\" TEXT NOT NULL DEFAULT '',\n    note TEXT DEFAULT 'it''s, ok', CHECK (telegram_id <> 0)\n, balance BIGINT NOT NULL DEFAULT\t0, ratio REAL DEFAULT -1.5)";
        let cols = columns(sql).unwrap();
        let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["telegram_id", "created at", "note", "balance", "ratio"]
        );
        assert!(cols[0].rowid);
        assert!(!cols[3].rowid);
        assert_eq!(cols[1].default.as_deref(), Some(""));
        assert_eq!(cols[2].default.as_deref(), Some("it's, ok"));
        assert_eq!(cols[3].default.as_deref(), Some("0"));
        assert_eq!(cols[4].default.as_deref(), Some("-1.5"));
        let keyed = columns("CREATE TABLE t (id INTEGER, code TEXT, PRIMARY KEY (id))").unwrap();
        assert!(keyed[0].rowid);
        let big = columns("CREATE TABLE t (id BIGINT PRIMARY KEY, code TEXT)").unwrap();
        assert!(!big[0].rowid);
        assert!(columns("CREATE TABLE t (a TEXT PRIMARY KEY) WITHOUT ROWID").is_none());
    }

    #[test]
    fn числа_записи_читаются_со_знаком() {
        let record = [
            7u8, 1, 2, 4, 9, 0, 7, 0xff, 0xfe, 0xd4, 0x00, 0x00, 0x00, 0x2a, 0x40, 0x09, 0x21,
            0xfb, 0x54, 0x44, 0x2d, 0x18,
        ];
        let values = record_values(&record).unwrap();
        assert!(matches!(values[0], Value::Int(-1)));
        assert!(matches!(values[1], Value::Int(-300)));
        assert!(matches!(values[2], Value::Int(42)));
        assert!(matches!(values[3], Value::Int(1)));
        assert!(matches!(values[4], Value::Null));
        assert!(matches!(values[5], Value::Real(v) if (v - std::f64::consts::PI).abs() < 1e-12));
    }
}
