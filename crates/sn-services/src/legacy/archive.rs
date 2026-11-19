use super::table::Dump;
use super::{chill_file, json_dump, pgcustom, pgdump, sqlite};
use flate2::read::MultiGzDecoder;
use std::fs::File;
use std::io::{BufReader, Cursor, Read};
use std::path::Path;

const PEEK: usize = 1024;
const READ_BUFFER: usize = 1 << 20;
const SIDE_FILE_MAX: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    NeedsPassword,
    WrongPassword,
    Unsupported(String),
    Io(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::NeedsPassword => f.write_str("архив защищён паролем"),
            OpenError::WrongPassword => f.write_str("пароль от архива не подошёл"),
            OpenError::Unsupported(text) | OpenError::Io(text) => f.write_str(text),
        }
    }
}

impl From<std::io::Error> for OpenError {
    fn from(err: std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::InvalidData
            || err.kind() == std::io::ErrorKind::UnexpectedEof
        {
            OpenError::Unsupported(
                "Файл повреждён или скачался не до конца. Скачайте выгрузку ещё раз".into(),
            )
        } else {
            OpenError::Io(format!("файл не прочитался: {err}"))
        }
    }
}

fn unsupported(text: &str) -> OpenError {
    OpenError::Unsupported(text.to_string())
}

fn read_head(reader: &mut dyn Read) -> std::io::Result<Vec<u8>> {
    let mut head = vec![0u8; PEEK];
    let mut filled = 0;
    while filled < PEEK {
        let n = reader.read(&mut head[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    head.truncate(filled);
    Ok(head)
}

fn is_json(head: &[u8]) -> bool {
    let head = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    head.iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|b| *b == b'{')
}

fn is_sql(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    trimmed.starts_with("--")
        || trimmed.starts_with("SET ")
        || trimmed.starts_with("COPY ")
        || trimmed.starts_with("CREATE ")
        || trimmed.starts_with("SELECT ")
        || trimmed.starts_with("\\connect")
}

struct Reader<'w> {
    wanted: &'w dyn Fn(&str) -> bool,
    dump: Dump,
    sqlite: bool,
}

impl Reader<'_> {
    fn stream(&mut self, mut reader: Box<dyn Read + '_>, depth: u8) -> Result<(), OpenError> {
        if depth > 4 {
            return Err(unsupported("Слишком много вложенных архивов в файле"));
        }
        let head = read_head(&mut reader)?;
        if head.is_empty() {
            return Err(unsupported("Файл пустой"));
        }
        let chained = Cursor::new(head.clone()).chain(reader);
        if head.starts_with(&[0x1f, 0x8b]) {
            return self.stream(Box::new(MultiGzDecoder::new(chained)), depth + 1);
        }
        if head.len() >= 262 && &head[257..262] == b"ustar" {
            return self.tar(Box::new(chained), depth);
        }
        if head.starts_with(b"PK\x03\x04") {
            return Err(unsupported(
                "Внутри архива ещё один zip. Распакуйте его и загрузите файл из него",
            ));
        }
        if head.starts_with(sqlite::MAGIC) {
            self.sqlite = true;
            return self.spool_sqlite(Box::new(chained));
        }
        if chill_file::is_sealed(&head) {
            return Err(unsupported(
                "Внутри архива зашифрованный бэкап Chill Remna bot. Загрузите сам файл .bak, без архива",
            ));
        }
        if is_json(&head) && chill_file::is_lines(&head) {
            chill_file::read_lines(
                BufReader::with_capacity(READ_BUFFER, chained),
                self.wanted,
                &mut self.dump,
            )?;
            return Ok(());
        }
        if head.starts_with(pgcustom::MAGIC) {
            self.dump.format = "custom";
            pgcustom::read(chained, self.wanted, &mut self.dump)?;
            return Ok(());
        }
        if is_json(&head) && String::from_utf8_lossy(&head).contains("-backup-parts\"") {
            return Err(unsupported(
                "Это список частей резервной копии, а не сама копия. Склейте части в один файл .zip или скачайте копию целиком в админке старого бота — как это сделать, написано в подсказке",
            ));
        }
        if is_json(&head) {
            self.dump.format = "json";
            json_dump::read(
                BufReader::with_capacity(READ_BUFFER, chained),
                self.wanted,
                &mut self.dump,
            )
            .map_err(|err| {
                if err.is_eof() {
                    unsupported("Файл обрывается на середине. Скачайте выгрузку ещё раз")
                } else {
                    unsupported("Файл похож на JSON, но прочитать его как выгрузку не получилось")
                }
            })?;
            return Ok(());
        }
        if is_sql(&head) {
            self.dump.format = "sql";
            pgdump::read(
                BufReader::with_capacity(READ_BUFFER, chained),
                self.wanted,
                &mut self.dump,
            )?;
            return Ok(());
        }
        Err(unsupported(
            "Это не выгрузка базы. Нужен файл резервной копии из старого бота или панели",
        ))
    }

    fn spool_sqlite(&mut self, mut reader: Box<dyn Read + '_>) -> Result<(), OpenError> {
        let dir = super::import_dir();
        std::fs::create_dir_all(&dir)?;
        let spool = Spool(dir.join(format!("sqlite-{}.part", uuid::Uuid::new_v4().simple())));
        let mut file = File::create(&spool.0)?;
        std::io::copy(&mut reader, &mut file)?;
        drop(file);
        sqlite::read(&spool.0, self.wanted, &mut self.dump)
    }

    fn tar(&mut self, reader: Box<dyn Read + '_>, depth: u8) -> Result<(), OpenError> {
        let mut archive = tar::Archive::new(reader);
        for entry in archive.entries()? {
            let entry = entry?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let path = entry.path()?.to_string_lossy().replace('\\', "/");
            let name = path.trim_start_matches("./").to_string();
            let inner = name
                .rsplit('/')
                .next()
                .unwrap_or(&name)
                .to_ascii_lowercase();
            let top = !name.contains('/');
            let database =
                inner.ends_with(".db") || inner.ends_with(".sqlite") || inner.ends_with(".sqlite3");
            if inner == "metadata.json" && top {
                json_dump::read_metadata(entry, &mut self.dump);
            } else if top
                && (inner.starts_with("database.")
                    || inner.ends_with(".sql")
                    || inner.ends_with(".sql.gz")
                    || inner.ends_with(".dump")
                    || inner.ends_with(".tar.gz")
                    || inner.ends_with(".tgz"))
                || database
            {
                self.stream(Box::new(entry), depth + 1)?;
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Dump, OpenError> {
        if self.dump.tables.is_empty() {
            if self.sqlite {
                return Err(unsupported(
                    "В файле базы SQLite нет таблиц ни одного из ботов, из которых умеет переносить панель",
                ));
            }
            return Err(unsupported(
                "В файле нет таблиц базы. Нужна полная резервная копия, а не отчёт или журнал",
            ));
        }
        Ok(self.dump)
    }
}

struct Spool(std::path::PathBuf);

impl Drop for Spool {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn zip_name_wanted(name: &str) -> bool {
    [
        ".tar.gz", ".tgz", ".tar", ".json", ".sql", ".dump", ".gz", ".bak", ".db", ".sqlite",
        ".sqlite3",
    ]
    .iter()
    .any(|ext| name.ends_with(ext))
}

fn zip_error(err: zip::result::ZipError) -> OpenError {
    match err {
        zip::result::ZipError::InvalidPassword => OpenError::WrongPassword,
        zip::result::ZipError::Io(err) => err.into(),
        zip::result::ZipError::UnsupportedArchive(_) => {
            unsupported("Такой zip-архив не открывается. Распакуйте его и загрузите файл из него")
        }
        _ => unsupported("Zip-архив повреждён. Скачайте выгрузку ещё раз"),
    }
}

struct Member {
    name: String,
    encrypted: bool,
    dir: bool,
}

fn listing(zip: &mut zip::ZipArchive<BufReader<File>>) -> Result<Vec<Member>, OpenError> {
    (0..zip.len())
        .map(|i| {
            let raw = zip.by_index_raw(i).map_err(zip_error)?;
            Ok(Member {
                name: raw
                    .name()
                    .map_err(zip_error)?
                    .replace('\\', "/")
                    .trim_start_matches("./")
                    .to_ascii_lowercase(),
                encrypted: raw.encrypted(),
                dir: raw.is_dir(),
            })
        })
        .collect()
}

fn bot_backup(members: &[Member]) -> bool {
    members.iter().any(|m| m.name == "manifest.json")
        && members
            .iter()
            .any(|m| !m.dir && m.name.starts_with("database/"))
}

fn open_bot_backup(
    zip: &mut zip::ZipArchive<BufReader<File>>,
    members: &[Member],
    password: Option<&str>,
    reader: &mut Reader<'_>,
) -> Result<(), OpenError> {
    let mut found = false;
    for (i, member) in members.iter().enumerate() {
        let name = member.name.as_str();
        let side =
            name == "manifest.json" || (name.starts_with("database/") && name.ends_with(".json"));
        let data = name.starts_with("database/")
            && (name.ends_with(".dump") || name.ends_with(".sql") || name.ends_with(".sql.gz"));
        if member.dir || !(side || data) {
            continue;
        }
        let entry = if member.encrypted {
            let Some(password) = password.filter(|p| !p.is_empty()) else {
                return Err(OpenError::NeedsPassword);
            };
            zip.by_index_decrypt(i, password.as_bytes())
                .map_err(zip_error)?
        } else {
            zip.by_index(i).map_err(zip_error)?
        };
        if side {
            let mut text = String::new();
            if entry.take(SIDE_FILE_MAX).read_to_string(&mut text).is_ok() {
                reader.dump.files.insert(name.to_string(), text);
            }
            continue;
        }
        found = true;
        reader.stream(Box::new(entry), 1)?;
    }
    if !found {
        return Err(unsupported(
            "В этой резервной копии нет выгрузки базы: при её создании выгрузка базы была выключена. Включите её в настройках резервных копий старого бота и сделайте копию заново",
        ));
    }
    Ok(())
}

fn open_zip(path: &Path, password: Option<&str>, reader: &mut Reader<'_>) -> Result<(), OpenError> {
    let mut zip = zip::ZipArchive::new(BufReader::new(File::open(path)?)).map_err(zip_error)?;
    let members = listing(&mut zip)?;
    if bot_backup(&members) {
        return open_bot_backup(&mut zip, &members, password, reader);
    }
    let mut found = false;
    for i in 0..zip.len() {
        let (encrypted, dir, name) = {
            let raw = zip.by_index_raw(i).map_err(zip_error)?;
            (
                raw.encrypted(),
                raw.is_dir(),
                raw.name().map_err(zip_error)?.to_ascii_lowercase(),
            )
        };
        let wanted = zip_name_wanted(&name);
        if dir || !wanted {
            continue;
        }
        found = true;
        let entry = if encrypted {
            let Some(password) = password.filter(|p| !p.is_empty()) else {
                return Err(OpenError::NeedsPassword);
            };
            zip.by_index_decrypt(i, password.as_bytes())
                .map_err(zip_error)?
        } else {
            zip.by_index(i).map_err(zip_error)?
        };
        reader.stream(Box::new(entry), 1)?;
    }
    if !found {
        return Err(unsupported(
            "В zip-архиве нет резервной копии. Загрузите файл, который прислал бот",
        ));
    }
    Ok(())
}

pub fn open(
    path: &Path,
    password: Option<&str>,
    wanted: &dyn Fn(&str) -> bool,
) -> Result<Dump, OpenError> {
    let mut file = File::open(path)?;
    let head = read_head(&mut file)?;
    let mut reader = Reader {
        wanted,
        dump: Dump::default(),
        sqlite: false,
    };
    if head.starts_with(sqlite::MAGIC) {
        reader.sqlite = true;
        sqlite::read(path, wanted, &mut reader.dump)?;
    } else if chill_file::is_sealed(&head) {
        return Err(OpenError::NeedsPassword);
    } else if head.starts_with(b"PK\x03\x04") {
        open_zip(path, password, &mut reader)?;
    } else {
        let file = File::open(path)?;
        reader.stream(Box::new(file), 0)?;
    }
    reader.finish()
}

pub fn extract_zip(
    path: &Path,
    password: Option<&str>,
) -> Result<Option<std::path::PathBuf>, OpenError> {
    let mut file = File::open(path)?;
    let head = read_head(&mut file)?;
    if !head.starts_with(b"PK\x03\x04") {
        return Ok(None);
    }
    let mut zip = zip::ZipArchive::new(BufReader::new(File::open(path)?)).map_err(zip_error)?;
    let members = listing(&mut zip)?;
    if bot_backup(&members) && members.iter().all(|m| !m.encrypted) {
        return Ok(None);
    }
    for i in 0..zip.len() {
        let (encrypted, dir, name) = {
            let raw = zip.by_index_raw(i).map_err(zip_error)?;
            (
                raw.encrypted(),
                raw.is_dir(),
                raw.name().map_err(zip_error)?.to_ascii_lowercase(),
            )
        };
        let wanted = zip_name_wanted(&name);
        if dir || !wanted {
            continue;
        }
        let mut entry = if encrypted {
            let Some(password) = password.filter(|p| !p.is_empty()) else {
                return Err(OpenError::NeedsPassword);
            };
            zip.by_index_decrypt(i, password.as_bytes())
                .map_err(zip_error)?
        } else {
            zip.by_index(i).map_err(zip_error)?
        };
        let target = path.with_extension("inner");
        let mut out = File::create(&target)?;
        if let Err(err) = std::io::copy(&mut entry, &mut out) {
            let _ = std::fs::remove_file(&target);
            return Err(if encrypted {
                OpenError::WrongPassword
            } else {
                err.into()
            });
        }
        return Ok(Some(target));
    }
    Err(unsupported(
        "В zip-архиве нет резервной копии. Загрузите файл, который прислал бот",
    ))
}

pub fn open_bytes(bytes: &[u8], wanted: &dyn Fn(&str) -> bool) -> Result<Dump, OpenError> {
    let mut reader = Reader {
        wanted,
        dump: Dump::default(),
        sqlite: false,
    };
    reader.stream(Box::new(Cursor::new(bytes.to_vec())), 0)?;
    reader.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    const SQL: &str = "--\n-- PostgreSQL database dump\n--\n\nCOPY public.clients (id, email) FROM stdin;\nc1\ta@example.com\n\\.\n";
    const JSON: &str = r#"{"metadata":{"timestamp":"2026-11-16T10:00:00+00:00"},"data":{"users":[{"id":1,"telegram_id":42}]}}"#;

    fn gz(bytes: &[u8]) -> Vec<u8> {
        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    fn tar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, *bytes).unwrap();
        }
        builder.into_inner().unwrap()
    }

    #[test]
    fn распознаются_sql_gz_tar_и_json() {
        let all = |_: &str| true;
        let plain = open_bytes(SQL.as_bytes(), &all).unwrap();
        assert_eq!(plain.count("clients"), 1);
        let gzipped = open_bytes(&gz(SQL.as_bytes()), &all).unwrap();
        assert_eq!(gzipped.count("clients"), 1);
        let archive = tar(&[
            (
                "metadata.json",
                br#"{"timestamp":"2026-11-16T10:00:00+00:00"}"#,
            ),
            ("database.json", JSON.as_bytes()),
            (
                "data/notes.sql",
                b"COPY public.junk (a) FROM stdin;\n1\n\\.\n",
            ),
        ]);
        let tarred = open_bytes(&gz(&archive), &all).unwrap();
        assert_eq!(tarred.count("users"), 1);
        assert!(tarred.table("junk").is_none());
        assert!(tarred.dumped_at.is_some());
        let sql_in_tar = open_bytes(&tar(&[("database.sql", SQL.as_bytes())]), &all).unwrap();
        assert_eq!(sql_in_tar.count("clients"), 1);
    }

    #[test]
    fn непонятный_файл_получает_понятный_ответ() {
        let all = |_: &str| true;
        assert!(matches!(
            open_bytes(b"\x89PNG....", &all),
            Err(OpenError::Unsupported(_))
        ));
        assert!(matches!(
            open_bytes(b"", &all),
            Err(OpenError::Unsupported(_))
        ));
        assert!(matches!(
            open_bytes(&tar(&[("database.sqlite", b"SQLite format 3\0....")]), &all),
            Err(OpenError::Unsupported(text)) if text.contains("SQLite")
        ));
        let mut cut = gz(SQL.repeat(200).as_bytes());
        cut.truncate(cut.len() / 2);
        assert!(matches!(
            open_bytes(&cut, &all),
            Err(OpenError::Unsupported(_))
        ));
    }
}
