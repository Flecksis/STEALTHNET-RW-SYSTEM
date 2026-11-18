use super::crypt::{self, CryptError, KdfParams, Opener, Sealer};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

pub const MANIFEST: &str = "backup.json";
pub const REMNA: &str = "remnawave.jsonl";
pub const SECRET_KEY: &str = "secret.key";
pub const SUMS: &str = "sums.json";
const DATA_PREFIX: &str = "data/";
const PART: usize = 8 * 1024 * 1024;
const READ_CHUNK: usize = 256 * 1024;
const SMALL_MAX: u64 = 64 * 1024 * 1024;
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Sealed,
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("это не файл копии STEALTHNET")]
    Foreign,
    #[error("копия зашифрована: нужен пароль")]
    PasswordNeeded,
    #[error(transparent)]
    Crypt(#[from] CryptError),
    #[error("файл копии повреждён: {0}")]
    Damaged(String),
    #[error("файл копии обрезан: он скачан или передан не до конца")]
    Truncated,
    #[error("{0}")]
    Io(io::Error),
    #[error("{0}")]
    Stopped(String),
}

impl From<io::Error> for ArchiveError {
    fn from(err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::UnexpectedEof {
            return ArchiveError::Truncated;
        }
        if err.kind() == io::ErrorKind::Interrupted
            && let Some(inner) = err.get_ref()
        {
            return ArchiveError::Stopped(inner.to_string());
        }
        if err.kind() == io::ErrorKind::InvalidData {
            let text = err.to_string();
            if text.contains("обрезан") {
                return ArchiveError::Truncated;
            }
            if text.contains("пароль не подошёл") {
                return ArchiveError::Crypt(CryptError::WrongPassword);
            }
            return ArchiveError::Damaged(text);
        }
        ArchiveError::Io(err)
    }
}

pub fn stop(reason: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, reason.into())
}

pub fn detect(path: &Path) -> Result<Kind, ArchiveError> {
    let mut head = [0u8; 8];
    let mut file = File::open(path)?;
    let mut filled = 0;
    while filled < head.len() {
        match file.read(&mut head[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    if filled >= 2 && head[..2] == GZIP_MAGIC {
        return Ok(Kind::Plain);
    }
    if filled == 8 && &head == crypt::MAGIC {
        return Ok(Kind::Sealed);
    }
    if filled == 0 {
        return Err(ArchiveError::Damaged("файл пустой".into()));
    }
    Err(ArchiveError::Foreign)
}

pub fn sealed_public(path: &Path) -> Result<Vec<u8>, ArchiveError> {
    let mut reader = BufReader::new(File::open(path)?);
    Ok(crypt::read_public(&mut reader)?)
}

struct Hashing<W: Write> {
    inner: W,
    hasher: Sha256,
    bytes: u64,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

enum Sink {
    Plain(Hashing<BufWriter<File>>),
    Sealed(Box<Sealer<Hashing<BufWriter<File>>>>),
}

impl Sink {
    fn finish(self) -> io::Result<Hashing<BufWriter<File>>> {
        match self {
            Sink::Plain(inner) => Ok(inner),
            Sink::Sealed(sealer) => sealer.finish(),
        }
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Sink::Plain(inner) => inner.write(buf),
            Sink::Sealed(inner) => inner.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Sink::Plain(inner) => inner.flush(),
            Sink::Sealed(inner) => inner.flush(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSum {
    pub sha256: String,
    pub bytes: u64,
    pub rows: u64,
    pub parts: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sums {
    #[serde(default)]
    pub tables: BTreeMap<usize, TableSum>,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

struct Current {
    index: usize,
    label: String,
    hasher: Sha256,
    buf: Vec<u8>,
    sum: TableSum,
}

pub struct Written {
    pub size: u64,
    pub sha256: String,
    pub sums: Sums,
}

pub struct Writer {
    tar: tar::Builder<GzEncoder<Sink>>,
    mtime: u64,
    current: Option<Current>,
    sums: Sums,
    last_index: Option<usize>,
}

pub struct SealWith<'a> {
    pub password: &'a str,
    pub params: KdfParams,
    pub public: &'a [u8],
}

impl Writer {
    pub fn create(path: &Path, seal: Option<SealWith<'_>>, mtime: u64) -> io::Result<Self> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        let hashing = Hashing {
            inner: BufWriter::with_capacity(1024 * 1024, file),
            hasher: Sha256::new(),
            bytes: 0,
        };
        let sink = match seal {
            None => Sink::Plain(hashing),
            Some(seal) => Sink::Sealed(Box::new(Sealer::new(
                hashing,
                seal.password,
                seal.params,
                seal.public,
            )?)),
        };
        let mut tar = tar::Builder::new(GzEncoder::new(sink, Compression::new(6)));
        tar.mode(tar::HeaderMode::Deterministic);
        Ok(Writer {
            tar,
            mtime,
            current: None,
            sums: Sums::default(),
            last_index: None,
        })
    }

    fn entry(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        header.set_mtime(self.mtime);
        header.set_entry_type(tar::EntryType::Regular);
        self.tar.append_data(&mut header, name, bytes)
    }

    pub fn file(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        self.close_table()?;
        self.sums
            .files
            .insert(name.to_string(), hex::encode(Sha256::digest(bytes)));
        self.entry(name, bytes)
    }

    pub fn begin_table(&mut self, index: usize, name: &str) -> io::Result<()> {
        self.close_table()?;
        if self.last_index.is_some_and(|last| index <= last) {
            return Err(io::Error::other("таблицы пишутся по порядку"));
        }
        self.last_index = Some(index);
        let label: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(48)
            .collect();
        self.current = Some(Current {
            index,
            label,
            hasher: Sha256::new(),
            buf: Vec::with_capacity(PART),
            sum: TableSum::default(),
        });
        Ok(())
    }

    pub fn table_data(&mut self, mut data: &[u8]) -> io::Result<()> {
        if self.current.is_none() {
            return Err(io::Error::other("данные без таблицы"));
        }
        while !data.is_empty() {
            let full = {
                let current = self.current.as_mut().expect("таблица открыта");
                let room = PART - current.buf.len();
                let take = room.min(data.len());
                let piece = &data[..take];
                current.hasher.update(piece);
                current.sum.bytes += take as u64;
                current.sum.rows += piece.iter().filter(|b| **b == b'\n').count() as u64;
                current.buf.extend_from_slice(piece);
                data = &data[take..];
                current.buf.len() == PART
            };
            if full {
                self.flush_part()?;
            }
        }
        Ok(())
    }

    fn flush_part(&mut self) -> io::Result<()> {
        let (name, buf) = {
            let current = self.current.as_mut().expect("таблица открыта");
            let name = if current.label.is_empty() {
                format!("{DATA_PREFIX}{:04}/{:06}", current.index, current.sum.parts)
            } else {
                format!(
                    "{DATA_PREFIX}{:04}-{}/{:06}",
                    current.index, current.label, current.sum.parts
                )
            };
            current.sum.parts += 1;
            (name, std::mem::take(&mut current.buf))
        };
        self.entry(&name, &buf)?;
        if let Some(current) = self.current.as_mut() {
            current.buf = buf;
            current.buf.clear();
        }
        Ok(())
    }

    fn close_table(&mut self) -> io::Result<()> {
        let Some(current) = self.current.as_ref() else {
            return Ok(());
        };
        if !current.buf.is_empty() || current.sum.parts == 0 {
            self.flush_part()?;
        }
        let current = self.current.take().expect("таблица открыта");
        let mut sum = current.sum;
        sum.sha256 = hex::encode(current.hasher.finalize());
        self.sums.tables.insert(current.index, sum);
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<Written> {
        self.close_table()?;
        let sums = serde_json::to_vec_pretty(&self.sums).map_err(io::Error::other)?;
        self.entry(SUMS, &sums)?;
        let gz = self.tar.into_inner()?;
        let sink = gz.finish()?;
        let hashing = sink.finish()?;
        let Hashing {
            inner,
            hasher,
            bytes,
        } = hashing;
        let file = inner.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        Ok(Written {
            size: bytes,
            sha256: hex::encode(hasher.finalize()),
            sums: self.sums,
        })
    }
}

pub trait Visitor {
    fn manifest(&mut self, raw: &[u8]) -> io::Result<()>;

    fn table_start(&mut self, _index: usize) -> io::Result<()> {
        Ok(())
    }

    fn table(&mut self, _index: usize, _chunk: &[u8]) -> io::Result<()> {
        Ok(())
    }

    fn table_end(&mut self, _index: usize) -> io::Result<()> {
        Ok(())
    }

    fn remna(&mut self, _chunk: &[u8]) -> io::Result<()> {
        Ok(())
    }

    fn secret_key(&mut self, _raw: &[u8]) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct ReadInfo {
    pub kind: Kind,
    pub public: Option<Vec<u8>>,
    pub file_sha256: String,
    pub file_size: u64,
    pub sums: Sums,
}

struct HashingReader<R: Read> {
    inner: R,
    hasher: Sha256,
    bytes: u64,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
}

fn data_entry(name: &str) -> Option<(usize, u32)> {
    let rest = name.strip_prefix(DATA_PREFIX)?;
    let (folder, part) = rest.split_once('/')?;
    let index = folder.get(..4)?;
    let label = &folder[4..];
    if !(label.is_empty() || label.starts_with('-'))
        || !index.bytes().all(|b| b.is_ascii_digit())
        || part.len() != 6
        || !part.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some((index.parse().ok()?, part.parse().ok()?))
}

fn read_small(entry: &mut impl Read, size: u64, name: &str) -> Result<Vec<u8>, ArchiveError> {
    if size > SMALL_MAX {
        return Err(ArchiveError::Damaged(format!(
            "слишком большой файл {name}"
        )));
    }
    let mut out = Vec::with_capacity(size as usize);
    entry.take(size).read_to_end(&mut out)?;
    if out.len() as u64 != size {
        return Err(ArchiveError::Truncated);
    }
    Ok(out)
}

pub fn read(
    path: &Path,
    password: Option<&str>,
    visitor: &mut dyn Visitor,
) -> Result<ReadInfo, ArchiveError> {
    let kind = detect(path)?;
    let mut hashing = HashingReader {
        inner: BufReader::with_capacity(1024 * 1024, File::open(path)?),
        hasher: Sha256::new(),
        bytes: 0,
    };
    let (sums, computed, public) = match kind {
        Kind::Plain => {
            let (sums, computed, rest) = walk(GzDecoder::new(&mut hashing), visitor)?;
            drain(rest)?;
            (sums, computed, None)
        }
        Kind::Sealed => {
            let Some(password) = password.filter(|p| !p.is_empty()) else {
                return Err(ArchiveError::PasswordNeeded);
            };
            let (opener, public) = Opener::new(&mut hashing, password)?;
            let (sums, computed, rest) = walk(GzDecoder::new(opener), visitor)?;
            drain(rest)?;
            (sums, computed, Some(public))
        }
    };
    std::io::copy(&mut hashing, &mut std::io::sink())?;
    for (index, sum) in &computed.tables {
        match sums.tables.get(index) {
            Some(expected) if expected == sum => {}
            _ => {
                return Err(ArchiveError::Damaged(format!(
                    "данные таблицы №{index} не сходятся с контрольной суммой"
                )));
            }
        }
    }
    if sums.tables.len() != computed.tables.len() {
        return Err(ArchiveError::Damaged("в копии не хватает таблиц".into()));
    }
    for (name, sha) in &computed.files {
        if sums.files.get(name) != Some(sha) {
            return Err(ArchiveError::Damaged(format!(
                "файл {name} не сходится с контрольной суммой"
            )));
        }
    }
    if sums
        .files
        .keys()
        .any(|name| !computed.files.contains_key(name))
    {
        return Err(ArchiveError::Damaged("в копии не хватает файлов".into()));
    }
    Ok(ReadInfo {
        kind,
        public,
        file_sha256: hex::encode(hashing.hasher.finalize()),
        file_size: hashing.bytes,
        sums,
    })
}

struct Open {
    index: usize,
    next_part: u32,
    hasher: Sha256,
    sum: TableSum,
}

fn drain(mut rest: impl Read) -> Result<(), ArchiveError> {
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        let got = rest.read(&mut buf)?;
        if got == 0 {
            return Ok(());
        }
        if buf[..got].iter().any(|b| *b != 0) {
            return Err(ArchiveError::Damaged(
                "после конца копии есть лишние данные".into(),
            ));
        }
    }
}

fn walk<R: Read>(source: R, visitor: &mut dyn Visitor) -> Result<(Sums, Sums, R), ArchiveError> {
    let mut archive = tar::Archive::new(source);
    let mut computed = Sums::default();
    let mut open: Option<Open> = None;
    let mut seen_manifest = false;
    let mut sums: Option<Sums> = None;
    let mut buf = vec![0u8; READ_CHUNK];
    let close = |open: &mut Option<Open>,
                 computed: &mut Sums,
                 visitor: &mut dyn Visitor|
     -> Result<(), ArchiveError> {
        if let Some(done) = open.take() {
            visitor.table_end(done.index)?;
            let mut sum = done.sum;
            sum.sha256 = hex::encode(done.hasher.finalize());
            computed.tables.insert(done.index, sum);
        }
        Ok(())
    };
    let entries = archive.entries()?;
    for entry in entries {
        let mut entry = entry?;
        if sums.is_some() {
            return Err(ArchiveError::Damaged(
                "после контрольных сумм есть данные".into(),
            ));
        }
        if entry.header().entry_type() != tar::EntryType::Regular {
            return Err(ArchiveError::Damaged("в копии есть не файл".into()));
        }
        let name = entry
            .path()
            .map_err(|_| ArchiveError::Damaged("неверное имя внутри копии".into()))?
            .to_string_lossy()
            .into_owned();
        let size = entry.size();
        if !seen_manifest {
            if name != MANIFEST {
                return Err(ArchiveError::Foreign);
            }
            let raw = read_small(&mut entry, size, &name)?;
            computed
                .files
                .insert(name.clone(), hex::encode(Sha256::digest(&raw)));
            visitor.manifest(&raw)?;
            seen_manifest = true;
            continue;
        }
        if let Some((index, part)) = data_entry(&name) {
            let continues = open.as_ref().is_some_and(|o| o.index == index);
            if !continues {
                if open.as_ref().is_some_and(|o| index <= o.index)
                    || computed
                        .tables
                        .keys()
                        .next_back()
                        .is_some_and(|last| index <= *last)
                {
                    return Err(ArchiveError::Damaged("таблицы идут не по порядку".into()));
                }
                close(&mut open, &mut computed, visitor)?;
                open = Some(Open {
                    index,
                    next_part: 0,
                    hasher: Sha256::new(),
                    sum: TableSum::default(),
                });
                visitor.table_start(index)?;
            }
            let current = open.as_mut().expect("таблица открыта");
            if part != current.next_part {
                return Err(ArchiveError::Damaged(
                    "части таблицы идут не по порядку".into(),
                ));
            }
            current.next_part += 1;
            current.sum.parts += 1;
            let mut left = size;
            while left > 0 {
                let want = (left as usize).min(buf.len());
                let got = entry.read(&mut buf[..want])?;
                if got == 0 {
                    return Err(ArchiveError::Truncated);
                }
                let chunk = &buf[..got];
                current.hasher.update(chunk);
                current.sum.bytes += got as u64;
                current.sum.rows += chunk.iter().filter(|b| **b == b'\n').count() as u64;
                visitor.table(index, chunk)?;
                left -= got as u64;
            }
            continue;
        }
        close(&mut open, &mut computed, visitor)?;
        match name.as_str() {
            REMNA => {
                let mut hasher = Sha256::new();
                let mut left = size;
                while left > 0 {
                    let want = (left as usize).min(buf.len());
                    let got = entry.read(&mut buf[..want])?;
                    if got == 0 {
                        return Err(ArchiveError::Truncated);
                    }
                    hasher.update(&buf[..got]);
                    visitor.remna(&buf[..got])?;
                    left -= got as u64;
                }
                computed.files.insert(name, hex::encode(hasher.finalize()));
            }
            SECRET_KEY => {
                let raw = read_small(&mut entry, size, &name)?;
                computed
                    .files
                    .insert(name, hex::encode(Sha256::digest(&raw)));
                visitor.secret_key(&raw)?;
            }
            SUMS => {
                let raw = read_small(&mut entry, size, &name)?;
                sums =
                    Some(serde_json::from_slice(&raw).map_err(|_| {
                        ArchiveError::Damaged("контрольные суммы не читаются".into())
                    })?);
            }
            _ => {
                std::io::copy(&mut entry, &mut std::io::sink())?;
            }
        }
    }
    close(&mut open, &mut computed, visitor)?;
    if !seen_manifest {
        return Err(ArchiveError::Truncated);
    }
    let sums = sums.ok_or(ArchiveError::Truncated)?;
    Ok((sums, computed, archive.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Collect {
        manifest: Vec<u8>,
        tables: BTreeMap<usize, Vec<u8>>,
        starts: Vec<usize>,
        ends: Vec<usize>,
        remna: Vec<u8>,
        key: Vec<u8>,
    }

    impl Visitor for Collect {
        fn manifest(&mut self, raw: &[u8]) -> io::Result<()> {
            self.manifest = raw.to_vec();
            Ok(())
        }

        fn table_start(&mut self, index: usize) -> io::Result<()> {
            self.starts.push(index);
            Ok(())
        }

        fn table(&mut self, index: usize, chunk: &[u8]) -> io::Result<()> {
            self.tables
                .entry(index)
                .or_default()
                .extend_from_slice(chunk);
            Ok(())
        }

        fn table_end(&mut self, index: usize) -> io::Result<()> {
            self.ends.push(index);
            Ok(())
        }

        fn remna(&mut self, chunk: &[u8]) -> io::Result<()> {
            self.remna.extend_from_slice(chunk);
            Ok(())
        }

        fn secret_key(&mut self, raw: &[u8]) -> io::Result<()> {
            self.key = raw.to_vec();
            Ok(())
        }
    }

    fn sample(path: &Path, seal: Option<SealWith<'_>>) -> Written {
        let mut writer = Writer::create(path, seal, 1_700_000_000).unwrap();
        writer
            .file(MANIFEST, b"{\"format\":\"stealthnet-backup\"}")
            .unwrap();
        writer.begin_table(0, "clients").unwrap();
        writer.table_data(b"1\tone\n2\ttwo\n").unwrap();
        writer.begin_table(1, "empty").unwrap();
        writer.begin_table(2, "big_table").unwrap();
        let big: Vec<u8> = (0..(PART + 1000))
            .map(|i| if i % 50 == 49 { b'\n' } else { b'x' })
            .collect();
        writer.table_data(&big[..1000]).unwrap();
        writer.table_data(&big[1000..]).unwrap();
        writer.file(REMNA, b"{\"source\":\"panel\"}\n").unwrap();
        writer.file(SECRET_KEY, b"a2V5").unwrap();
        writer.finish().unwrap()
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sn-archive-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn архив_пишется_и_читается_с_проверкой_сумм() {
        let path = temp("a.tar.gz");
        let written = sample(&path, None);
        assert_eq!(written.sums.tables.len(), 3);
        assert_eq!(written.sums.tables[&0].rows, 2);
        assert_eq!(written.sums.tables[&1].parts, 1);
        assert_eq!(written.sums.tables[&1].bytes, 0);
        assert_eq!(written.sums.tables[&2].parts, 2);
        let mut collect = Collect::default();
        let info = read(&path, None, &mut collect).unwrap();
        assert_eq!(info.kind, Kind::Plain);
        assert_eq!(info.file_sha256, written.sha256);
        assert_eq!(info.file_size, written.size);
        assert_eq!(collect.starts, vec![0, 1, 2]);
        assert_eq!(collect.ends, vec![0, 1, 2]);
        assert_eq!(collect.tables[&0], b"1\tone\n2\ttwo\n");
        assert_eq!(collect.tables[&2].len(), PART + 1000);
        assert_eq!(collect.remna, b"{\"source\":\"panel\"}\n");
        assert_eq!(collect.key, b"a2V5");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), written.size);
    }

    #[test]
    fn зашифрованный_архив_требует_пароль() {
        let path = temp("a.tar.gz.enc");
        sample(
            &path,
            Some(SealWith {
                password: "secret",
                params: KdfParams::QUICK,
                public: b"{}",
            }),
        );
        assert_eq!(detect(&path).unwrap(), Kind::Sealed);
        assert!(matches!(
            read(&path, None, &mut Collect::default()),
            Err(ArchiveError::PasswordNeeded)
        ));
        assert!(matches!(
            read(&path, Some("wrong"), &mut Collect::default()),
            Err(ArchiveError::Crypt(CryptError::WrongPassword))
        ));
        let mut collect = Collect::default();
        let info = read(&path, Some("secret"), &mut collect).unwrap();
        assert_eq!(info.kind, Kind::Sealed);
        assert_eq!(info.public.as_deref(), Some(&b"{}"[..]));
        assert_eq!(collect.tables[&0], b"1\tone\n2\ttwo\n");
    }

    #[test]
    fn битый_и_обрезанный_архив_распознаются() {
        let path = temp("a.tar.gz");
        sample(&path, None);
        let bytes = std::fs::read(&path).unwrap();
        let cut = temp("cut.tar.gz");
        std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
        let err = read(&cut, None, &mut Collect::default()).unwrap_err();
        assert!(matches!(err, ArchiveError::Truncated), "{err}");
        let broken = temp("broken.tar.gz");
        let mut damaged = bytes.clone();
        let at = damaged.len() / 3;
        damaged[at] ^= 0xff;
        std::fs::write(&broken, &damaged).unwrap();
        assert!(read(&broken, None, &mut Collect::default()).is_err());
        let foreign = temp("foreign.zip");
        std::fs::write(&foreign, b"PK\x03\x04 zip").unwrap();
        assert!(matches!(
            read(&foreign, None, &mut Collect::default()),
            Err(ArchiveError::Foreign)
        ));
        let empty = temp("empty");
        std::fs::write(&empty, b"").unwrap();
        assert!(read(&empty, None, &mut Collect::default()).is_err());
    }

    #[test]
    fn чужой_tar_gz_не_принимается_за_копию() {
        let path = temp("other.tar.gz");
        let file = File::create(&path).unwrap();
        let mut tar = tar::Builder::new(GzEncoder::new(file, Compression::fast()));
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(0o600);
        tar.append_data(&mut header, "readme.txt", &b"hi"[..])
            .unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        assert!(matches!(
            read(&path, None, &mut Collect::default()),
            Err(ArchiveError::Foreign)
        ));
    }

    #[test]
    fn имена_частей_разбираются_строго() {
        assert_eq!(data_entry("data/0003/000012"), Some((3, 12)));
        assert_eq!(data_entry("data/0003-clients/000012"), Some((3, 12)));
        assert_eq!(data_entry("data/0003clients/000012"), None);
        assert_eq!(data_entry("data/3/12"), None);
        assert_eq!(data_entry("data/0003"), None);
        assert_eq!(data_entry("backup.json"), None);
    }
}
