use super::manifest::{self, ImageFile, Manifest, PLATFORMS, Problem, Run};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use zip::{CompressionMethod, ZipArchive};

pub const PACKAGE_MAX_BYTES: u64 = 500 * 1024 * 1024;
pub const MANIFEST_FILE: &str = "module.json";
pub const SIGNATURE_FILE: &str = "module.sig";
pub const IMAGE_REPO: &str = "stealthnet-module";
pub const SPEC_FILE: &str = "spec";
pub const IMAGE_OUT: &str = "image.tar";
const SIGNATURE_MAX: u64 = 4 * 1024;
const ENTRIES_MAX: usize = 4;
const IMAGE_META_MAX: u64 = 4 * 1024 * 1024;
const IMAGE_ENTRIES_MAX: usize = 20_000;
const ALGORITHM: &str = "ed25519";

pub const TRUSTED: &[(&str, &str)] =
    &[("STEALTHNET", "rl/uj0iBodBVzHrQ4nLkCJHebuu8a+A1Pc5C8QqX9dQ=")];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Signature {
    Verified { signer: String, fingerprint: String },
    Unknown { fingerprint: String },
    Unsigned,
}

impl Signature {
    pub const fn verified(&self) -> bool {
        matches!(self, Signature::Verified { .. })
    }

    pub fn fingerprint(&self) -> Option<&str> {
        match self {
            Signature::Verified { fingerprint, .. } | Signature::Unknown { fingerprint } => {
                Some(fingerprint)
            }
            Signature::Unsigned => None,
        }
    }

    pub fn signer(&self) -> Option<&str> {
        match self {
            Signature::Verified { signer, .. } => Some(signer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspected {
    pub manifest: Manifest,
    pub signature: Signature,
    pub platforms: Vec<String>,
    pub image: ImageFile,
    pub archive_sha256: String,
    pub archive_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub module: String,
    pub image: String,
    pub run: Run,
}

impl Spec {
    pub fn render(&self) -> String {
        let mut out = format!(
            "module={}\nimage={}\nport={}\nhealth={}\nmemory={}\ncpus={}\ninternet={}\n",
            self.module,
            self.image,
            self.run.port,
            self.run.health_path,
            self.run.memory_mb,
            self.run.cpus_text(),
            u8::from(self.run.internet),
        );
        for volume in &self.run.volumes {
            out.push_str(&format!("volume={}:{}\n", volume.name, volume.path));
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packed {
    pub path: PathBuf,
    pub bytes: u64,
    pub platforms: Vec<String>,
    pub fingerprint: Option<String>,
}

fn problem(path: &str, message: impl Into<String>) -> Problem {
    Problem {
        path: path.to_string(),
        message: message.into(),
    }
}

fn fail(path: &str, message: impl Into<String>) -> Vec<Problem> {
    vec![problem(path, message)]
}

pub fn first_message(problems: &[Problem]) -> String {
    problems
        .first()
        .map(|p| {
            if p.path.is_empty() {
                p.message.clone()
            } else {
                format!("{}: {}", p.path, p.message)
            }
        })
        .unwrap_or_else(|| "архив модуля не прошёл проверку".into())
}

pub fn platform_for_arch(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" | "amd64" => Some("linux/amd64"),
        "aarch64" | "arm64" => Some("linux/arm64"),
        _ => None,
    }
}

pub fn host_platform() -> Option<&'static str> {
    platform_for_arch(std::env::consts::ARCH)
}

pub fn processor(platform: &str) -> &'static str {
    match platform {
        "linux/amd64" => "x86_64",
        "linux/arm64" => "ARM64",
        _ => "неизвестный процессор",
    }
}

pub fn fingerprint(key: &[u8]) -> String {
    let digest = Sha256::digest(key);
    hex::encode_upper(&digest[..8])
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn image_tag(module: &str, sha256: &str) -> String {
    let short: String = sha256.chars().take(12).collect();
    format!("{IMAGE_REPO}/{module}:{short}")
}

fn allowed_name(name: &str) -> bool {
    name == MANIFEST_FILE || name == SIGNATURE_FILE || PLATFORMS.iter().any(|(_, f)| *f == name)
}

fn allowed_list() -> String {
    let mut names = vec![MANIFEST_FILE, SIGNATURE_FILE];
    names.extend(PLATFORMS.iter().map(|(_, f)| *f));
    names.join(", ")
}

fn shown(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(80).collect()
}

struct Limited<R> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for Limited<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::other("данных больше, чем заявлено")),
            };
        }
        let want = buf
            .len()
            .min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let got = self.inner.read(&mut buf[..want])?;
        self.left -= got as u64;
        Ok(got)
    }
}

fn limited<R: Read>(inner: R, limit: u64) -> Limited<R> {
    Limited { inner, left: limit }
}

fn read_all(reader: impl Read, limit: u64) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    limited(reader, limit).read_to_end(&mut out)?;
    Ok(out)
}

fn hash_reader(reader: impl Read, limit: u64) -> io::Result<(String, u64)> {
    let mut reader = limited(reader, limit);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let got = reader.read(&mut buf)?;
        if got == 0 {
            break;
        }
        hasher.update(&buf[..got]);
        total += got as u64;
    }
    Ok((hex::encode(hasher.finalize()), total))
}

pub fn hash_file(path: &Path) -> io::Result<(String, u64)> {
    hash_reader(BufReader::new(File::open(path)?), u64::MAX)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignatureFile {
    algorithm: String,
    key: String,
    signature: String,
}

fn decode_key(raw: &str) -> Option<Vec<u8>> {
    STANDARD
        .decode(raw.trim())
        .ok()
        .filter(|key| key.len() == 32)
}

pub fn check_signature(
    raw: &[u8],
    message: &[u8],
    trusted: &[(&str, &str)],
) -> Result<Signature, String> {
    let file: SignatureFile = serde_json::from_slice(raw)
        .map_err(|_| "файл подписи не разобран: соберите архив командой stealthnet module pack")?;
    if file.algorithm != ALGORITHM {
        return Err(format!(
            "неизвестный способ подписи «{}», нужен ed25519",
            shown(&file.algorithm)
        ));
    }
    let key = decode_key(&file.key).ok_or("ключ подписи — 32 байта в base64")?;
    let signature = STANDARD
        .decode(file.signature.trim())
        .ok()
        .filter(|s| s.len() == 64)
        .ok_or("подпись — 64 байта в base64")?;
    UnparsedPublicKey::new(&ED25519, &key)
        .verify(message, &signature)
        .map_err(|_| "подпись не совпадает с module.json: архив изменили после подписи")?;
    let fingerprint = fingerprint(&key);
    Ok(
        match trusted
            .iter()
            .find(|(_, k)| decode_key(k).as_deref() == Some(key.as_slice()))
        {
            Some((signer, _)) => Signature::Verified {
                signer: (*signer).to_string(),
                fingerprint,
            },
            None => Signature::Unknown { fingerprint },
        },
    )
}

type Zip = ZipArchive<BufReader<File>>;

fn open_zip(path: &Path) -> Result<(Zip, u64), Vec<Problem>> {
    let size = std::fs::metadata(path)
        .map_err(|_| fail("", "архив не прочитан"))?
        .len();
    if size == 0 {
        return Err(fail("", "файл пустой"));
    }
    if size > PACKAGE_MAX_BYTES {
        return Err(fail("", "архив больше 500 МБ"));
    }
    let file = File::open(path).map_err(|_| fail("", "архив не прочитан"))?;
    let zip = ZipArchive::new(BufReader::new(file))
        .map_err(|_| fail("", "это не zip-архив или он повреждён"))?;
    Ok((zip, size))
}

struct Entry {
    name: String,
    index: usize,
    size: u64,
}

fn entries(zip: &mut Zip) -> Result<Vec<Entry>, Vec<Problem>> {
    if zip.len() > ENTRIES_MAX {
        return Err(fail(
            "",
            format!(
                "в архиве {} файлов, а в архиве модуля бывают только {}",
                zip.len(),
                allowed_list()
            ),
        ));
    }
    let mut out: Vec<Entry> = Vec::new();
    let mut problems = Vec::new();
    for index in 0..zip.len() {
        let raw = zip
            .by_index_raw(index)
            .map_err(|_| fail("", "архив повреждён"))?;
        let Ok(name) = raw.name().map(|n| n.to_string()) else {
            problems.push(problem("", "имя файла в архиве не прочитано"));
            continue;
        };
        let label = shown(&name);
        if raw.is_dir() {
            problems.push(problem(&label, "в архиве модуля не бывает папок"));
        } else if raw.is_symlink() {
            problems.push(problem(&label, "ссылки в архиве модуля запрещены"));
        } else if raw.encrypted() {
            problems.push(problem(&label, "архив с паролем не подходит"));
        } else if !matches!(
            raw.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            problems.push(problem(
                &label,
                "файл сжат неизвестным способом: соберите архив командой stealthnet module pack",
            ));
        } else if !allowed_name(&name) {
            problems.push(problem(
                &label,
                format!(
                    "лишний файл: в архиве модуля бывают только {}",
                    allowed_list()
                ),
            ));
        } else if out.iter().any(|e| e.name == name) {
            problems.push(problem(&label, "файл в архиве повторяется"));
        } else {
            out.push(Entry {
                size: raw.size(),
                name,
                index,
            });
        }
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems)
    }
}

fn read_entry(zip: &mut Zip, index: usize, limit: u64) -> io::Result<Vec<u8>> {
    let entry = zip.by_index(index).map_err(io::Error::other)?;
    read_all(entry, limit)
}

pub fn inspect(
    path: &Path,
    platform: &str,
    trusted: &[(&str, &str)],
) -> Result<Inspected, Vec<Problem>> {
    let (mut zip, size) = open_zip(path)?;
    let list = entries(&mut zip)?;
    let find = |name: &str| list.iter().find(|e| e.name == name);
    let Some(entry) = find(MANIFEST_FILE) else {
        return Err(fail(
            "",
            "в архиве нет module.json: это не архив модуля STEALTHNET",
        ));
    };
    let limit = manifest::MAX_BYTES as u64;
    if entry.size > limit {
        return Err(fail(MANIFEST_FILE, "больше 64 КБ"));
    }
    let bytes = read_entry(&mut zip, entry.index, limit)
        .map_err(|_| fail(MANIFEST_FILE, "файл не прочитан: архив повреждён"))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| fail(MANIFEST_FILE, "файл не в кодировке UTF-8"))?;
    let manifest = manifest::parse_package(text)?;
    let signature = match find(SIGNATURE_FILE) {
        None => Signature::Unsigned,
        Some(entry) => {
            if entry.size > SIGNATURE_MAX {
                return Err(fail(SIGNATURE_FILE, "подпись больше 4 КБ"));
            }
            let raw = read_entry(&mut zip, entry.index, SIGNATURE_MAX)
                .map_err(|_| fail(SIGNATURE_FILE, "подпись не прочитана"))?;
            check_signature(&raw, &bytes, trusted).map_err(|m| fail(SIGNATURE_FILE, m))?
        }
    };
    let Some(run) = manifest.run.as_ref() else {
        return Err(fail("run", "в архиве модуля нужен раздел run"));
    };
    let mut problems = Vec::new();
    for image in &run.images {
        let at = format!("run.images.{}", image.platform);
        match find(&image.file) {
            None => problems.push(problem(&at, format!("в архиве нет файла {}", image.file))),
            Some(entry) if entry.size != image.bytes => problems.push(problem(
                &at,
                format!(
                    "в архиве {} занимает {} байт, а в module.json записано {}",
                    image.file, entry.size, image.bytes
                ),
            )),
            Some(_) => {}
        }
    }
    for entry in &list {
        if entry.name != MANIFEST_FILE
            && entry.name != SIGNATURE_FILE
            && !run.images.iter().any(|i| i.file == entry.name)
        {
            problems.push(problem(
                &entry.name,
                "образ не описан в module.json: соберите архив командой stealthnet module pack",
            ));
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let platforms: Vec<String> = run.images.iter().map(|i| i.platform.clone()).collect();
    let Some(image) = run.image(platform).cloned() else {
        return Err(fail(
            "run.images",
            format!(
                "в архиве нет сборки для процессора этого сервера ({}). В архиве есть: {}",
                processor(platform),
                platforms
                    .iter()
                    .map(|p| processor(p))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    };
    let Some(entry) = find(&image.file) else {
        return Err(fail(&image.file, "образа нет в архиве"));
    };
    let reader = zip
        .by_index(entry.index)
        .map_err(|_| fail(&image.file, "образ не прочитан: архив повреждён"))?;
    let (sum, count) = hash_reader(reader, image.bytes)
        .map_err(|_| fail(&image.file, "образ не прочитан: архив повреждён"))?;
    if count != image.bytes || sum != image.sha256 {
        return Err(fail(
            &image.file,
            "контрольная сумма образа не совпадает с module.json: архив повреждён или изменён",
        ));
    }
    let (archive_sha256, _) = hash_file(path).map_err(|_| fail("", "архив не прочитан"))?;
    Ok(Inspected {
        manifest,
        signature,
        platforms,
        image,
        archive_sha256,
        archive_bytes: size,
    })
}

fn retag_manifest(data: &[u8], tag: &str) -> Result<Vec<u8>, String> {
    let mut value: Value =
        serde_json::from_slice(data).map_err(|_| "manifest.json образа не разобран")?;
    let items = value
        .as_array_mut()
        .filter(|items| items.len() == 1)
        .ok_or("в архиве образа должен быть ровно один образ")?;
    let image = items[0]
        .as_object_mut()
        .ok_or("manifest.json образа не разобран")?;
    image.insert(
        "RepoTags".into(),
        Value::Array(vec![Value::String(tag.into())]),
    );
    serde_json::to_vec(&value).map_err(|e| e.to_string())
}

fn retag_index(data: &[u8], tag: &str) -> Result<Vec<u8>, String> {
    let mut value: Value =
        serde_json::from_slice(data).map_err(|_| "index.json образа не разобран")?;
    let manifests = value
        .get_mut("manifests")
        .and_then(Value::as_array_mut)
        .filter(|items| items.len() == 1)
        .ok_or("в архиве образа должен быть ровно один образ")?;
    let entry = manifests[0]
        .as_object_mut()
        .ok_or("index.json образа не разобран")?;
    let mut annotations = entry
        .get("annotations")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    annotations.retain(|key, _| {
        key != "org.opencontainers.image.ref.name" && key != "io.containerd.image.name"
    });
    let short = tag.rsplit(':').next().unwrap_or(tag);
    annotations.insert(
        "io.containerd.image.name".into(),
        Value::String(format!("docker.io/{tag}")),
    );
    annotations.insert(
        "org.opencontainers.image.ref.name".into(),
        Value::String(short.into()),
    );
    entry.insert("annotations".into(), Value::Object(annotations));
    serde_json::to_vec(&value).map_err(|e| e.to_string())
}

pub fn normalize(reader: impl Read, writer: impl Write, tag: &str) -> Result<(), String> {
    let mut archive = tar::Archive::new(reader);
    let mut builder = tar::Builder::new(writer);
    let mut described = false;
    let mut count = 0usize;
    let broken = |e: io::Error| format!("архив образа повреждён: {e}");
    for entry in archive.entries().map_err(broken)? {
        let mut entry = entry.map_err(broken)?;
        count += 1;
        if count > IMAGE_ENTRIES_MAX {
            return Err("в образе слишком много файлов".into());
        }
        let path = entry.path().map_err(broken)?.into_owned();
        let name = path.to_string_lossy();
        let name = name.trim_start_matches("./").to_string();
        let kind = entry.header().entry_type();
        let mut header = entry.header().clone();
        if kind.is_pax_global_extensions() {
            continue;
        }
        if name == "repositories" {
            continue;
        }
        if (name == "manifest.json" || name == "index.json") && kind.is_file() {
            let data = read_all(&mut entry, IMAGE_META_MAX).map_err(broken)?;
            let data = if name == "manifest.json" {
                retag_manifest(&data, tag)?
            } else {
                retag_index(&data, tag)?
            };
            described = true;
            header.set_size(data.len() as u64);
            builder
                .append_data(&mut header, &path, data.as_slice())
                .map_err(broken)?;
            continue;
        }
        if kind.is_file() || kind.is_dir() {
            builder
                .append_data(&mut header, &path, &mut entry)
                .map_err(broken)?;
        } else if kind.is_symlink() || kind.is_hard_link() {
            let target = entry
                .link_name()
                .map_err(broken)?
                .ok_or("в образе ссылка без цели")?
                .into_owned();
            builder
                .append_link(&mut header, &path, &target)
                .map_err(broken)?;
        } else {
            return Err(format!(
                "в образе запись неизвестного типа: {}",
                shown(&name)
            ));
        }
    }
    if !described {
        return Err("это не образ из docker save: нет manifest.json".into());
    }
    builder
        .into_inner()
        .and_then(|mut w| w.flush())
        .map_err(|e| format!("образ не записан: {e}"))
}

fn create_new(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    options.open(path)
}

pub fn unpack(package: &Path, out: &Path, platform: &str) -> Result<Spec, String> {
    let inspected = inspect(package, platform, TRUSTED).map_err(|p| first_message(&p))?;
    let (mut zip, _) = open_zip(package).map_err(|p| first_message(&p))?;
    let list = entries(&mut zip).map_err(|p| first_message(&p))?;
    let entry = list
        .iter()
        .find(|e| e.name == inspected.image.file)
        .ok_or("образа нет в архиве")?;
    let reader = zip
        .by_index(entry.index)
        .map_err(|_| "образ не прочитан: архив повреждён")?;
    let module = inspected.manifest.id.clone();
    let tag = image_tag(&module, &inspected.image.sha256);
    let file = create_new(&out.join(IMAGE_OUT)).map_err(|e| format!("образ не записан: {e}"))?;
    normalize(
        limited(reader, inspected.image.bytes),
        BufWriter::new(file),
        &tag,
    )?;
    let run = inspected.manifest.run.clone().ok_or("нет раздела run")?;
    let spec = Spec {
        module,
        image: tag,
        run,
    };
    let mut file =
        create_new(&out.join(SPEC_FILE)).map_err(|e| format!("описание не записано: {e}"))?;
    file.write_all(spec.render().as_bytes())
        .and_then(|()| file.flush())
        .map_err(|e| format!("описание не записано: {e}"))?;
    Ok(spec)
}

pub fn keygen(path: &Path) -> Result<(String, String), String> {
    let rng = ring::rand::SystemRandom::new();
    let document = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "ключ не создан")?;
    let pair = Ed25519KeyPair::from_pkcs8(document.as_ref()).map_err(|_| "ключ не создан")?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("файл ключа не создан ({}): {e}", path.display()))?;
    file.write_all(format!("{}\n", STANDARD.encode(document.as_ref())).as_bytes())
        .and_then(|()| file.flush())
        .map_err(|e| format!("ключ не записан: {e}"))?;
    let public = pair.public_key().as_ref();
    Ok((STANDARD.encode(public), fingerprint(public)))
}

pub fn read_key(path: &Path) -> Result<Ed25519KeyPair, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("ключ не прочитан ({}): {e}", path.display()))?;
    let raw = STANDARD
        .decode(text.trim())
        .map_err(|_| "ключ не разобран: ожидается файл из stealthnet module keygen")?;
    Ed25519KeyPair::from_pkcs8(&raw)
        .map_err(|_| "ключ не разобран: ожидается файл из stealthnet module keygen".into())
}

pub fn sign(pair: &Ed25519KeyPair, message: &[u8]) -> String {
    let value = serde_json::json!({
        "algorithm": ALGORITHM,
        "key": STANDARD.encode(pair.public_key().as_ref()),
        "signature": STANDARD.encode(pair.sign(message).as_ref()),
    });
    format!("{value}\n")
}

fn safe_version(version: &str) -> String {
    version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn pack(dir: &Path, key: Option<&Path>, out: Option<&Path>) -> Result<Packed, String> {
    let source_path = dir.join(MANIFEST_FILE);
    let source =
        std::fs::read(&source_path).map_err(|_| format!("нет файла {}", source_path.display()))?;
    if source.len() > manifest::MAX_BYTES {
        return Err("module.json больше 64 КБ".into());
    }
    let mut value: Value =
        serde_json::from_slice(&source).map_err(|e| format!("module.json — не JSON: {e}"))?;
    let run = value
        .get_mut("run")
        .and_then(Value::as_object_mut)
        .ok_or("в module.json нет раздела run")?;
    let mut images = Map::new();
    for (platform, file) in PLATFORMS {
        let path = dir.join(file);
        if !path.is_file() {
            continue;
        }
        let (sha256, bytes) = hash_file(&path).map_err(|e| format!("{file} не прочитан: {e}"))?;
        images.insert(
            platform.to_string(),
            serde_json::json!({ "file": file, "sha256": sha256, "bytes": bytes }),
        );
    }
    if images.is_empty() {
        return Err(format!(
            "рядом с module.json нет образа: сохраните его командой docker save -o {} <образ>",
            PLATFORMS[0].1
        ));
    }
    run.insert("images".into(), Value::Object(images));
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
    );
    let manifest = manifest::parse_package(&text).map_err(|problems| {
        problems
            .iter()
            .map(|p| {
                if p.path.is_empty() {
                    p.message.clone()
                } else {
                    format!("{}: {}", p.path, p.message)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let run = manifest
        .run
        .clone()
        .ok_or("в module.json нет раздела run")?;
    let pair = key.map(read_key).transpose()?;
    let signature = pair.as_ref().map(|p| sign(p, text.as_bytes()));
    let out = out.map(Path::to_path_buf).unwrap_or_else(|| {
        PathBuf::from(format!(
            "{}-{}.zip",
            manifest.id,
            safe_version(&manifest.version)
        ))
    });
    let file = create_new(&out).map_err(|e| format!("архив не создан ({}): {e}", out.display()))?;
    let written = write_zip(file, dir, &text, signature.as_deref(), &run);
    if let Err(err) = written {
        let _ = std::fs::remove_file(&out);
        return Err(err);
    }
    let own_key = pair
        .as_ref()
        .map(|p| STANDARD.encode(p.public_key().as_ref()));
    let mut trusted: Vec<(&str, &str)> = TRUSTED.to_vec();
    if let Some(own) = own_key.as_deref() {
        trusted.push(("ключ сборки", own));
    }
    for image in &run.images {
        if let Err(problems) = inspect(&out, &image.platform, &trusted) {
            let _ = std::fs::remove_file(&out);
            return Err(format!(
                "собранный архив не прошёл проверку: {}",
                first_message(&problems)
            ));
        }
    }
    let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    Ok(Packed {
        path: out,
        bytes,
        platforms: run.images.iter().map(|i| i.platform.clone()).collect(),
        fingerprint: pair.as_ref().map(|p| fingerprint(p.public_key().as_ref())),
    })
}

fn write_zip(
    file: File,
    dir: &Path,
    manifest: &str,
    signature: Option<&str>,
    run: &Run,
) -> Result<(), String> {
    let failed = |e: zip::result::ZipError| format!("архив не записан: {e}");
    let io_failed = |e: io::Error| format!("архив не записан: {e}");
    let mut writer = zip::ZipWriter::new(BufWriter::new(file));
    let small = zip::write::SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    writer.start_file(MANIFEST_FILE, small).map_err(failed)?;
    writer.write_all(manifest.as_bytes()).map_err(io_failed)?;
    if let Some(signature) = signature {
        writer.start_file(SIGNATURE_FILE, small).map_err(failed)?;
        writer.write_all(signature.as_bytes()).map_err(io_failed)?;
    }
    for image in &run.images {
        let options = small.large_file(image.bytes >= u64::from(u32::MAX));
        writer
            .start_file(image.file.as_str(), options)
            .map_err(failed)?;
        let mut source = File::open(dir.join(&image.file)).map_err(io_failed)?;
        io::copy(&mut source, &mut writer).map_err(io_failed)?;
    }
    writer.finish().map_err(failed)?.flush().map_err(io_failed)
}

#[cfg(test)]
#[path = "package_tests.rs"]
mod tests;
