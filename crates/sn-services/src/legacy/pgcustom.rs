use super::archive::OpenError;
use super::pgdump::{copy_header, decode_field};
use super::table::{Dump, Table};
use chrono::{DateTime, TimeZone, Utc};
use flate2::read::ZlibDecoder;
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};

pub const MAGIC: &[u8] = b"PGDMP";

const READ_BUFFER: usize = 1 << 20;
const MAX_TEXT: i64 = 256 * 1024 * 1024;
const MAX_KEPT: usize = 512 * 1024 * 1024;
const MAX_ENTRIES: i64 = 10_000_000;
const BLOCK_DATA: u8 = 1;
const BLOCK_BLOBS: u8 = 3;

const fn version(major: u32, minor: u32) -> u32 {
    (major << 16) | (minor << 8)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Packing {
    Plain,
    Zlib,
    Lz4,
    Zstd,
}

struct Replay<R> {
    inner: R,
    kept: Vec<u8>,
    at: usize,
    keep: bool,
}

impl<R: Read> Replay<R> {
    fn rewind(&mut self) -> bool {
        if !self.keep {
            return false;
        }
        self.at = 0;
        true
    }

    fn release(&mut self) {
        self.keep = false;
        if self.at >= self.kept.len() {
            self.kept = Vec::new();
            self.at = 0;
        }
    }
}

impl<R: Read> Read for Replay<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.at < self.kept.len() {
            let n = out.len().min(self.kept.len() - self.at);
            out[..n].copy_from_slice(&self.kept[self.at..self.at + n]);
            self.at += n;
            if !self.keep && self.at == self.kept.len() {
                self.kept = Vec::new();
                self.at = 0;
            }
            return Ok(n);
        }
        let n = self.inner.read(out)?;
        if self.keep {
            if self.kept.len() + n > MAX_KEPT {
                self.keep = false;
                self.kept = Vec::new();
                self.at = 0;
            } else {
                self.kept.extend_from_slice(&out[..n]);
                self.at = self.kept.len();
            }
        }
        Ok(n)
    }
}

struct Archive<R> {
    src: Replay<R>,
    version: u32,
    int_size: usize,
    off_size: usize,
    packing: Packing,
    created: Option<DateTime<Utc>>,
}

fn broken() -> OpenError {
    OpenError::Unsupported(
        "Копия pg_dump повреждена или сделана незнакомой версией pg_dump. Сделайте копию заново или выгрузите обычный файл .sql: pg_dump -Fp".into(),
    )
}

fn cut() -> OpenError {
    OpenError::Unsupported(
        "Копия pg_dump -Fc скачалась не до конца или повреждена. Скачайте её ещё раз или выгрузите обычный файл .sql".into(),
    )
}

fn io_error(err: std::io::Error) -> OpenError {
    if err.kind() == std::io::ErrorKind::UnexpectedEof {
        cut()
    } else {
        OpenError::from(err)
    }
}

fn packed(packing: Packing) -> OpenError {
    let name = if packing == Packing::Lz4 {
        "lz4"
    } else {
        "zstd"
    };
    OpenError::Unsupported(format!(
        "Копия pg_dump сжата способом {name}, его перенос не читает. Сделайте копию заново без --compress или выгрузите обычный файл .sql"
    ))
}

impl<R: Read> Archive<R> {
    fn exact(&mut self, buf: &mut [u8]) -> Result<(), OpenError> {
        self.src.read_exact(buf).map_err(io_error)
    }

    fn byte(&mut self) -> Result<u8, OpenError> {
        let mut one = [0u8; 1];
        self.exact(&mut one)?;
        Ok(one[0])
    }

    fn int(&mut self) -> Result<i64, OpenError> {
        let sign = if self.version > version(1, 0) {
            self.byte()?
        } else {
            0
        };
        let mut value: i64 = 0;
        for at in 0..self.int_size {
            let b = i64::from(self.byte()?);
            if at < 7 {
                value |= b << (8 * at);
            } else if b != 0 {
                return Err(broken());
            }
        }
        Ok(if sign != 0 { -value } else { value })
    }

    fn text(&mut self) -> Result<Option<String>, OpenError> {
        let len = self.int()?;
        if len < 0 {
            return Ok(None);
        }
        if len > MAX_TEXT {
            return Err(broken());
        }
        let mut buf = vec![0u8; len as usize];
        self.exact(&mut buf)?;
        Ok(Some(match String::from_utf8(buf) {
            Ok(text) => text,
            Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
        }))
    }

    fn offset(&mut self) -> Result<(), OpenError> {
        if self.version < version(1, 7) {
            self.int()?;
            return Ok(());
        }
        if !(1..=3).contains(&self.byte()?) {
            return Err(broken());
        }
        for _ in 0..self.off_size {
            self.byte()?;
        }
        Ok(())
    }

    fn open(reader: R) -> Result<Self, OpenError> {
        let mut archive = Archive {
            src: Replay {
                inner: reader,
                kept: Vec::new(),
                at: 0,
                keep: false,
            },
            version: version(1, 0),
            int_size: 4,
            off_size: 8,
            packing: Packing::Zlib,
            created: None,
        };
        let mut magic = [0u8; 5];
        archive.exact(&mut magic)?;
        if magic != *MAGIC {
            return Err(broken());
        }
        let major = u32::from(archive.byte()?);
        let minor = u32::from(archive.byte()?);
        let revision = if major > 1 || (major == 1 && minor > 0) {
            u32::from(archive.byte()?)
        } else {
            0
        };
        archive.version = version(major, minor) | revision;
        if archive.version < version(1, 12) {
            return Err(OpenError::Unsupported(
                "Копия сделана очень старой версией pg_dump (до PostgreSQL 9.0). Выгрузите базу обычным файлом .sql: pg_dump -Fp".into(),
            ));
        }
        if major > 1 {
            return Err(broken());
        }
        archive.int_size = usize::from(archive.byte()?);
        if !(1..=8).contains(&archive.int_size) {
            return Err(broken());
        }
        archive.off_size = usize::from(archive.byte()?);
        if !(1..=16).contains(&archive.off_size) {
            return Err(broken());
        }
        match archive.byte()? {
            1 => {}
            3 | 5 => {
                return Err(OpenError::Unsupported(
                    "Это копия pg_dump из нескольких файлов (формат tar или каталог). Сделайте копию одним файлом: pg_dump -Fc или обычный .sql".into(),
                ));
            }
            _ => return Err(broken()),
        }
        archive.packing = if archive.version >= version(1, 15) {
            match archive.byte()? {
                0 => Packing::Plain,
                1 => Packing::Zlib,
                2 => Packing::Lz4,
                3 => Packing::Zstd,
                _ => return Err(broken()),
            }
        } else if archive.int()? != 0 {
            Packing::Zlib
        } else {
            Packing::Plain
        };
        let mut moment = [0i64; 7];
        for slot in &mut moment {
            *slot = archive.int()?;
        }
        let [sec, min, hour, day, month, year, _] = moment;
        archive.created = Utc
            .with_ymd_and_hms(
                i32::try_from(year + 1900).unwrap_or(0),
                u32::try_from(month + 1).unwrap_or(0),
                u32::try_from(day).unwrap_or(0),
                u32::try_from(hour).unwrap_or(0),
                u32::try_from(min).unwrap_or(0),
                u32::try_from(sec).unwrap_or(0),
            )
            .single();
        archive.text()?;
        archive.text()?;
        archive.text()?;
        Ok(archive)
    }

    fn toc(&mut self, relkind: bool) -> Result<HashMap<i64, (String, Vec<String>)>, OpenError> {
        let count = self.int()?;
        if !(0..=MAX_ENTRIES).contains(&count) {
            return Err(broken());
        }
        let mut entries = HashMap::new();
        for _ in 0..count {
            let id = self.int()?;
            if id <= 0 {
                return Err(broken());
            }
            self.int()?;
            self.text()?;
            self.text()?;
            self.text()?;
            let desc = self.text()?.unwrap_or_default();
            if desc.is_empty()
                || !desc
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b' ' || b == b'_')
            {
                return Err(broken());
            }
            self.int()?;
            self.text()?;
            self.text()?;
            let copy = self.text()?;
            self.text()?;
            self.text()?;
            if self.version >= version(1, 14) {
                self.text()?;
            }
            if relkind {
                self.int()?;
            }
            self.text()?;
            if !matches!(self.text()?.as_deref(), Some("false" | "true")) {
                return Err(broken());
            }
            let mut deps = 0;
            while self.text()?.is_some() {
                deps += 1;
                if deps > 1_000_000 {
                    return Err(broken());
                }
            }
            self.offset()?;
            if desc == "TABLE DATA"
                && let Some(header) = copy.as_deref().and_then(|c| copy_header(c.trim()))
            {
                entries.insert(id, (header.table, header.columns));
            }
        }
        Ok(entries)
    }

    fn skip(&mut self) -> Result<(), OpenError> {
        loop {
            let len = self.int()?;
            if len == 0 {
                return Ok(());
            }
            if len < 0 {
                return Err(broken());
            }
            let len = len as u64;
            let copied = std::io::copy(&mut (&mut self.src).take(len), &mut std::io::sink())?;
            if copied != len {
                return Err(cut());
            }
        }
    }

    fn load(&mut self, columns: &[String]) -> Result<Table, OpenError> {
        let packing = self.packing;
        if matches!(packing, Packing::Lz4 | Packing::Zstd) {
            return Err(packed(packing));
        }
        let mut table = Table::new(columns.to_vec());
        let mut chunks = Chunks {
            archive: self,
            left: 0,
            done: false,
            failed: None,
        };
        let outcome = {
            let reader: Box<dyn Read + '_> = if packing == Packing::Zlib {
                Box::new(ZlibDecoder::new(&mut chunks))
            } else {
                Box::new(&mut chunks)
            };
            rows(BufReader::with_capacity(READ_BUFFER, reader), &mut table)
        };
        if let Some(err) = chunks.failed.take() {
            return Err(err);
        }
        outcome.map_err(|err| {
            if err.kind() == std::io::ErrorKind::UnexpectedEof {
                cut()
            } else {
                broken()
            }
        })?;
        while !chunks.done {
            std::io::copy(&mut chunks, &mut std::io::sink()).map_err(io_error)?;
            if let Some(err) = chunks.failed.take() {
                return Err(err);
            }
        }
        Ok(table)
    }
}

struct Chunks<'a, R> {
    archive: &'a mut Archive<R>,
    left: u64,
    done: bool,
    failed: Option<OpenError>,
}

impl<R: Read> Read for Chunks<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.left == 0 {
            if self.done {
                return Ok(0);
            }
            match self.archive.int() {
                Ok(0) => {
                    self.done = true;
                    return Ok(0);
                }
                Ok(len) if len > 0 => self.left = len as u64,
                Ok(_) => {
                    self.failed = Some(broken());
                    return Err(std::io::Error::other("broken block"));
                }
                Err(err) => {
                    self.failed = Some(err);
                    return Err(std::io::Error::other("broken block"));
                }
            }
        }
        let want = out
            .len()
            .min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let n = self.archive.src.read(&mut out[..want])?;
        if n == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

fn rows(mut reader: impl BufRead, table: &mut Table) -> std::io::Result<()> {
    let mut buffer = Vec::with_capacity(64 * 1024);
    loop {
        buffer.clear();
        if reader.read_until(b'\n', &mut buffer)? == 0 {
            return Ok(());
        }
        while matches!(buffer.last(), Some(b'\n' | b'\r')) {
            buffer.pop();
        }
        if buffer == b"\\." {
            continue;
        }
        let line = String::from_utf8_lossy(&buffer);
        let values: Vec<Option<Cow<'_, str>>> = line.split('\t').map(decode_field).collect();
        table.push(values.iter().map(|v| v.as_deref()));
    }
}

fn merge(dump: &mut Dump, name: String, table: Table) {
    let target = dump.tables.entry(name).or_default();
    if target.columns().is_empty() {
        *target = table;
        return;
    }
    for row in table.rows() {
        let values = table
            .columns()
            .iter()
            .map(|c| (c.clone(), row.raw(c).map(str::to_string)));
        target.push_named(values);
    }
}

pub fn read(
    reader: impl Read,
    wanted: &dyn Fn(&str) -> bool,
    dump: &mut Dump,
) -> Result<usize, OpenError> {
    let mut archive = Archive::open(BufReader::with_capacity(READ_BUFFER, reader))?;
    let newer = archive.version >= version(1, 16);
    archive.src.keep = newer;
    let entries = match archive.toc(newer) {
        Ok(entries) => entries,
        Err(first) => {
            if !newer || !archive.src.rewind() {
                return Err(first);
            }
            archive.toc(false).map_err(|_| first)?
        }
    };
    archive.src.release();
    if dump.dumped_at.is_none() {
        dump.dumped_at = archive.created;
    }
    let mut loaded = 0usize;
    loop {
        let mut kind = [0u8; 1];
        if archive.src.read(&mut kind)? == 0 {
            break;
        }
        let id = archive.int()?;
        match kind[0] {
            BLOCK_DATA => match entries.get(&id).filter(|(name, _)| wanted(name)) {
                Some((name, columns)) => {
                    let table = archive.load(columns)?;
                    merge(dump, name.clone(), table);
                    loaded += 1;
                }
                None => archive.skip()?,
            },
            BLOCK_BLOBS => loop {
                if archive.int()? == 0 {
                    break;
                }
                archive.skip()?;
            },
            _ => return Err(broken()),
        }
    }
    Ok(loaded)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    pub struct Builder {
        pub minor: u8,
        pub zlib: bool,
        pub relkind: bool,
        out: Vec<u8>,
        entries: Vec<(i64, String, String)>,
        blocks: Vec<(u8, i64, Vec<u8>)>,
    }

    fn int(out: &mut Vec<u8>, value: i64) {
        out.push(u8::from(value < 0));
        out.extend_from_slice(&(value.unsigned_abs() as u32).to_le_bytes());
    }

    fn text(out: &mut Vec<u8>, value: Option<&str>) {
        match value {
            None => int(out, -1),
            Some(v) => {
                int(out, v.len() as i64);
                out.extend_from_slice(v.as_bytes());
            }
        }
    }

    impl Builder {
        pub fn new(minor: u8, zlib: bool) -> Self {
            Builder {
                minor,
                zlib,
                relkind: minor >= 16,
                out: Vec::new(),
                entries: Vec::new(),
                blocks: Vec::new(),
            }
        }

        pub fn table(&mut self, id: i64, name: &str, columns: &[&str], rows: &[&str]) {
            self.entries.push((
                id,
                "TABLE DATA".into(),
                format!("COPY public.{name} ({}) FROM stdin;\n", columns.join(", ")),
            ));
            let mut body = String::new();
            for row in rows {
                body.push_str(row);
                body.push('\n');
            }
            self.blocks.push((BLOCK_DATA, id, body.into_bytes()));
        }

        pub fn schema(&mut self, id: i64, desc: &str) {
            self.entries.push((id, desc.into(), String::new()));
        }

        pub fn blobs(&mut self, id: i64) {
            self.entries.push((id, "BLOBS".into(), String::new()));
            self.blocks
                .push((BLOCK_BLOBS, id, b"large object".to_vec()));
        }

        fn data(&self, out: &mut Vec<u8>, bytes: &[u8]) {
            let packed = if self.zlib {
                let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
                enc.write_all(bytes).unwrap();
                enc.finish().unwrap()
            } else {
                bytes.to_vec()
            };
            for chunk in packed.chunks(7) {
                int(out, chunk.len() as i64);
                out.extend_from_slice(chunk);
            }
            int(out, 0);
        }

        pub fn build(mut self) -> Vec<u8> {
            let mut out = std::mem::take(&mut self.out);
            out.extend_from_slice(MAGIC);
            out.extend_from_slice(&[1, self.minor, 0, 4, 8, 1]);
            if self.minor >= 15 {
                out.push(u8::from(self.zlib));
            } else {
                int(&mut out, if self.zlib { -1 } else { 0 });
            }
            for value in [12, 30, 9, 18, 10, 126, 0] {
                int(&mut out, value);
            }
            text(&mut out, Some("shop"));
            text(&mut out, Some("16.4"));
            text(&mut out, Some("16.4"));
            int(&mut out, self.entries.len() as i64);
            for (id, desc, copy) in &self.entries {
                int(&mut out, *id);
                int(&mut out, i64::from(desc == "TABLE DATA"));
                text(&mut out, Some("0"));
                text(&mut out, Some("0"));
                text(&mut out, Some("t"));
                text(&mut out, Some(desc));
                int(&mut out, 2);
                text(&mut out, Some(""));
                text(&mut out, Some(""));
                text(&mut out, (!copy.is_empty()).then_some(copy.as_str()));
                text(&mut out, Some("public"));
                text(&mut out, Some(""));
                if self.minor >= 14 {
                    text(&mut out, Some("heap"));
                }
                if self.relkind {
                    int(&mut out, i64::from(b'r'));
                }
                text(&mut out, Some("postgres"));
                text(&mut out, Some("false"));
                text(&mut out, Some("1"));
                text(&mut out, None);
                out.push(if desc == "TABLE DATA" { 2 } else { 3 });
                out.extend_from_slice(&[0u8; 8]);
            }
            let blocks = std::mem::take(&mut self.blocks);
            for (kind, id, bytes) in blocks {
                out.push(kind);
                int(&mut out, id);
                if kind == BLOCK_BLOBS {
                    int(&mut out, 16_400);
                    self.data(&mut out, &bytes);
                    int(&mut out, 0);
                } else {
                    self.data(&mut out, &bytes);
                }
            }
            out
        }
    }

    fn sample(minor: u8, zlib: bool, relkind: bool) -> Vec<u8> {
        let mut builder = Builder::new(minor, zlib);
        builder.relkind = relkind;
        builder.schema(1, "SCHEMA");
        builder.table(
            2,
            "users",
            &["id", "name", "note"],
            &["1\tАнна\t\\N", "2\tБорис\tстрока\\nвторая"],
        );
        builder.blobs(3);
        builder.table(4, "noise", &["a"], &["x"]);
        builder.build()
    }

    #[test]
    fn данные_custom_формата_читаются_во_всех_версиях() {
        for (minor, zlib, relkind) in [
            (14, true, false),
            (14, false, false),
            (15, true, false),
            (15, false, false),
            (16, true, true),
            (16, true, false),
        ] {
            let mut dump = Dump::default();
            let loaded = read(
                sample(minor, zlib, relkind).as_slice(),
                &|t| t == "users",
                &mut dump,
            )
            .unwrap();
            assert_eq!(loaded, 1, "1.{minor}");
            let rows: Vec<_> = dump.rows("users").collect();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].text("name"), Some("Анна"));
            assert_eq!(rows[0].text("note"), None);
            assert_eq!(rows[1].raw("note"), Some("строка\nвторая"));
            assert!(dump.table("noise").is_none());
            assert_eq!(
                dump.dumped_at.map(|d| d.to_rfc3339()),
                Some("2026-11-18T09:30:12+00:00".to_string())
            );
        }
    }

    #[test]
    fn обрезанный_и_чужой_файл_дают_понятную_ошибку() {
        let whole = sample(15, true, false);
        let cut = &whole[..whole.len() - 20];
        let mut dump = Dump::default();
        assert!(matches!(
            read(cut, &|_| true, &mut dump),
            Err(OpenError::Unsupported(text)) if text.contains("не до конца")
        ));
        let mut tar = whole.clone();
        tar[10] = 3;
        assert!(matches!(
            read(tar.as_slice(), &|_| true, &mut Dump::default()),
            Err(OpenError::Unsupported(text)) if text.contains("формат tar")
        ));
        let mut lz4 = whole.clone();
        lz4[11] = 2;
        assert!(matches!(
            read(lz4.as_slice(), &|_| true, &mut Dump::default()),
            Err(OpenError::Unsupported(text)) if text.contains("lz4")
        ));
        let mut garbage = whole;
        garbage.truncate(40);
        garbage.extend_from_slice(&[0xff; 64]);
        assert!(read(garbage.as_slice(), &|_| true, &mut Dump::default()).is_err());
    }
}
