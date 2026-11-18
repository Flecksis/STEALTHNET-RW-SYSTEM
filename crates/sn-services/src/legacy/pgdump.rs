use super::table::{Dump, Table, parse_time};
use std::borrow::Cow;
use std::io::BufRead;

pub struct CopyHeader {
    pub table: String,
    pub columns: Vec<String>,
}

fn unquote(name: &str) -> String {
    let name = name.trim();
    match name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) {
        Some(inner) => inner.replace("\"\"", "\""),
        None => name.to_ascii_lowercase(),
    }
}

fn split_identifiers(list: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in list.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ',' if !quoted => {
                parts.push(unquote(&current));
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        parts.push(unquote(&current));
    }
    parts
}

pub fn copy_header(line: &str) -> Option<CopyHeader> {
    let rest = line.strip_prefix("COPY ")?;
    let open = rest.find('(')?;
    let close = rest.rfind(')')?;
    if close < open || !rest[close..].contains("FROM stdin") {
        return None;
    }
    let qualified = rest[..open].trim();
    let name = split_qualified(qualified);
    Some(CopyHeader {
        table: name,
        columns: split_identifiers(&rest[open + 1..close]),
    })
}

fn split_qualified(qualified: &str) -> String {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in qualified.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            '.' if !quoted => {
                parts.push(unquote(&current));
                current.clear();
            }
            _ => current.push(c),
        }
    }
    parts.push(unquote(&current));
    parts.pop().unwrap_or_default()
}

pub fn decode_field(raw: &str) -> Option<Cow<'_, str>> {
    if raw == "\\N" {
        return None;
    }
    if !raw.contains('\\') {
        return Some(Cow::Borrowed(raw));
    }
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'\\' || i + 1 >= bytes.len() {
            out.push(b);
            i += 1;
            continue;
        }
        let next = bytes[i + 1];
        i += 2;
        match next {
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(0x0b),
            b'0'..=b'7' => {
                let mut value = u32::from(next - b'0');
                let mut taken = 0;
                while taken < 2 && i < bytes.len() && (b'0'..=b'7').contains(&bytes[i]) {
                    value = value * 8 + u32::from(bytes[i] - b'0');
                    i += 1;
                    taken += 1;
                }
                out.push((value & 0xff) as u8);
            }
            b'x' => {
                let mut value = 0u32;
                let mut taken = 0;
                while taken < 2 && i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                    value = value * 16 + (bytes[i] as char).to_digit(16).unwrap_or(0);
                    i += 1;
                    taken += 1;
                }
                if taken == 0 {
                    out.push(b'x');
                } else {
                    out.push((value & 0xff) as u8);
                }
            }
            other => out.push(other),
        }
    }
    Some(Cow::Owned(String::from_utf8_lossy(&out).into_owned()))
}

fn created_at(line: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let rest = line.strip_prefix("-- ")?;
    let (_, value) = rest.split_once("Created at:")?;
    parse_time(value.trim())
}

pub fn read(
    mut reader: impl BufRead,
    wanted: &dyn Fn(&str) -> bool,
    dump: &mut Dump,
) -> std::io::Result<usize> {
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut copies = 0usize;
    let mut current: Option<(String, Option<Table>)> = None;
    let mut header_lines = 0usize;
    loop {
        buffer.clear();
        let read = reader.read_until(b'\n', &mut buffer)?;
        if read == 0 {
            break;
        }
        while matches!(buffer.last(), Some(b'\n' | b'\r')) {
            buffer.pop();
        }
        if let Some((name, table)) = current.as_mut() {
            if buffer == b"\\." {
                let name = std::mem::take(name);
                if let Some(table) = current.take().and_then(|(_, t)| t) {
                    let target = dump.tables.entry(name).or_default();
                    if target.columns().is_empty() {
                        *target = table;
                    } else {
                        for row in table.rows() {
                            let values = table
                                .columns()
                                .iter()
                                .map(|c| (c.clone(), row.raw(c).map(str::to_string)));
                            target.push_named(values);
                        }
                    }
                } else {
                    current = None;
                }
                continue;
            }
            if let Some(table) = table.as_mut() {
                let line = String::from_utf8_lossy(&buffer);
                let values: Vec<Option<Cow<'_, str>>> =
                    line.split('\t').map(decode_field).collect();
                table.push(values.iter().map(|v| v.as_deref()));
            }
            continue;
        }
        if buffer.starts_with(b"COPY ") {
            let line = String::from_utf8_lossy(&buffer);
            if let Some(header) = copy_header(&line) {
                copies += 1;
                let table = wanted(&header.table).then(|| Table::new(header.columns));
                current = Some((header.table, table));
            }
            continue;
        }
        if header_lines < 64 && buffer.starts_with(b"--") {
            header_lines += 1;
            if dump.dumped_at.is_none() {
                dump.dumped_at = created_at(&String::from_utf8_lossy(&buffer));
            }
        }
    }
    Ok(copies)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "-- StealthNet v7.0.1 Database Dump\n-- Created at: 2026-11-17T09:12:44.000Z\n\nSET statement_timeout = 0;\n\
COPY public.clients (id, email, \"telegramId\", balance) FROM stdin;\n\
c1\tann@example.com\t1001\t12.5\n\
c2\t\\N\t1002\t0\n\
\\.\n\
COPY public.webhook_events (id, body) FROM stdin;\n\
w1\t{\"a\":1}\n\
\\.\n\
COPY public.ticket_messages (id, content) FROM stdin;\n\
m1\tстрока\\nвторая\\tтаб \\\\ слеш\n\
\\.\n";

    #[test]
    fn copy_блоки_разбираются_а_ненужные_пропускаются() {
        let mut dump = Dump::default();
        let copies = read(SAMPLE.as_bytes(), &|t| t != "webhook_events", &mut dump).unwrap();
        assert_eq!(copies, 3);
        assert!(dump.table("webhook_events").is_none());
        let clients: Vec<_> = dump.rows("clients").collect();
        assert_eq!(clients.len(), 2);
        assert_eq!(clients[0].text("email"), Some("ann@example.com"));
        assert_eq!(clients[0].i64("telegramId"), Some(1001));
        assert_eq!(clients[1].text("email"), None);
        let message = dump.rows("ticket_messages").next().unwrap();
        assert_eq!(message.raw("content"), Some("строка\nвторая\tтаб \\ слеш"));
        assert_eq!(
            dump.dumped_at.map(|d| d.to_rfc3339()),
            Some("2026-11-17T09:12:44+00:00".to_string())
        );
    }

    #[test]
    fn заголовок_copy_с_кавычками_и_схемой() {
        let header =
            copy_header("COPY \"public\".\"Client Data\" (\"id\", plain, \"Mixed\") FROM stdin;")
                .unwrap();
        assert_eq!(header.table, "Client Data");
        assert_eq!(header.columns, ["id", "plain", "Mixed"]);
        assert!(copy_header("COPY public.t (a) TO stdout;").is_none());
    }

    #[test]
    fn экранирование_восьмеричное_и_шестнадцатеричное() {
        assert_eq!(decode_field("a\\101\\x42c").as_deref(), Some("aABc"));
        assert_eq!(decode_field("\\N"), None);
    }
}
