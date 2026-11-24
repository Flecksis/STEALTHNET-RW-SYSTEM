use crate::updates::{UpdateConfig, read_helper, read_small};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const HOST_PREFIX: &str = "sn-mod-";
pub const PANEL_HOST: &str = "stealthnet-panel";
const UPLOAD_PREFIX: &str = "pkg-";
const UPLOAD_SUFFIX: &str = ".zip";
const UPLOAD_KEEP: std::time::Duration = std::time::Duration::from_secs(3600);
pub const REQUEST_EXPIRES_SECONDS: i64 = 600;
const LOG_LINES: usize = 200;
const LOG_LINE_MAX: usize = 500;
const MESSAGE_MAX: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Ready,
    NoHelper,
    HelperOff,
    Outdated,
    ModulesOff,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HelperState {
    pub state: Availability,
    pub why: Option<String>,
    pub busy: bool,
}

impl HelperState {
    pub fn ready(&self) -> bool {
        self.state == Availability::Ready
    }

    pub fn reason(&self) -> String {
        match self.state {
            Availability::Ready => String::new(),
            Availability::NoHelper => match self.why.as_deref() {
                Some("no_root") => "панель установлена без прав root, поэтому помощника на сервере нет".into(),
                Some("no_systemd") => "на сервере нет systemd, поэтому помощника нет".into(),
                Some("path") => "в пути к папке системы есть пробелы или особые знаки, поэтому помощника нет".into(),
                Some("owner") => "папку системы может менять не только root, поэтому помощника нет".into(),
                Some("systemd_failed") => "systemd не принял службу помощника".into(),
                _ => "помощника на сервере нет".into(),
            },
            Availability::HelperOff => "помощник на сервере выключен командой stealthnet helper off".into(),
            Availability::Outdated => "помощник на сервере не умеет ставить модули: выполните один раз sudo stealthnet update".into(),
            Availability::ModulesOff => "модули из админки на сервере запрещены командой stealthnet modules off".into(),
        }
    }
}

pub fn valid_module(id: &str) -> bool {
    (2..=40).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn helper_state(config: Option<&UpdateConfig>) -> HelperState {
    let why = config.and_then(|c| c.helper_why.clone());
    let Some(dir) = config.and_then(|c| c.dir.as_deref()) else {
        return HelperState {
            state: Availability::NoHelper,
            why,
            busy: false,
        };
    };
    let busy = request_path(dir).exists();
    let state = match read_helper(dir) {
        None => Availability::NoHelper,
        Some(file) => match file.modules {
            Some(true) => Availability::Ready,
            Some(false) if !file.panel_updates => Availability::HelperOff,
            Some(false) => Availability::ModulesOff,
            None if !file.panel_updates => Availability::HelperOff,
            None => Availability::Outdated,
        },
    };
    HelperState { state, why, busy }
}

pub fn request_path(dir: &Path) -> PathBuf {
    dir.join("request").join("request.json")
}

pub fn upload_path(dir: &Path, upload: &Uuid) -> PathBuf {
    dir.join("request")
        .join(format!("{UPLOAD_PREFIX}{upload}{UPLOAD_SUFFIX}"))
}

fn credentials_path(dir: &Path, request: &Uuid) -> PathBuf {
    dir.join("request").join(format!("{request}.cred"))
}

pub fn cleanup_uploads(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir.join("request")) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(UPLOAD_PREFIX) && name.ends_with(UPLOAD_SUFFIX)) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| now.duration_since(at).ok())
            .is_some_and(|age| age > UPLOAD_KEEP);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    ModuleInstall,
    ModuleUpdate,
    ModuleStart,
    ModuleStop,
    ModuleRestart,
    ModuleLogs,
    ModuleSecrets,
    ModuleRemove,
}

impl Action {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Action::ModuleInstall => "module-install",
            Action::ModuleUpdate => "module-update",
            Action::ModuleStart => "module-start",
            Action::ModuleStop => "module-stop",
            Action::ModuleRestart => "module-restart",
            Action::ModuleLogs => "module-logs",
            Action::ModuleSecrets => "module-secrets",
            Action::ModuleRemove => "module-remove",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [
            Action::ModuleInstall,
            Action::ModuleUpdate,
            Action::ModuleStart,
            Action::ModuleStop,
            Action::ModuleRestart,
            Action::ModuleLogs,
            Action::ModuleSecrets,
            Action::ModuleRemove,
        ]
        .into_iter()
        .find(|a| a.as_str() == raw)
    }

    pub const fn steps(&self) -> &'static [&'static str] {
        match self {
            Action::ModuleInstall | Action::ModuleUpdate => &["check", "load", "start", "verify"],
            Action::ModuleStart | Action::ModuleRestart | Action::ModuleSecrets => {
                &["start", "verify"]
            }
            Action::ModuleStop => &["stop"],
            Action::ModuleLogs => &["logs"],
            Action::ModuleRemove => &["stop", "clean"],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: Uuid,
    pub action: Action,
    pub module: String,
    pub package: Option<Uuid>,
    pub erase: Option<bool>,
    pub admin_id: i64,
    pub at: DateTime<Utc>,
}

impl Request {
    pub fn line(&self) -> String {
        let at = self.at.to_rfc3339_opts(SecondsFormat::Secs, true);
        let middle = match (self.action, self.package, self.erase) {
            (Action::ModuleInstall | Action::ModuleUpdate, Some(package), _) => {
                format!(",\"package\":\"{package}\"")
            }
            (Action::ModuleRemove, _, Some(erase)) => {
                format!(",\"data\":\"{}\"", if erase { "erase" } else { "keep" })
            }
            _ => String::new(),
        };
        format!(
            "{{\"id\":\"{}\",\"action\":\"{}\",\"module\":\"{}\"{middle},\"requested_at\":\"{at}\",\"admin_id\":{}}}\n",
            self.id,
            self.action.as_str(),
            self.module,
            self.admin_id
        )
    }

    pub fn marker(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "action": self.action.as_str(),
            "requested_at": self.at,
            "admin_id": self.admin_id,
        })
    }
}

async fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    use tokio::io::AsyncWriteExt;
    let written = async {
        file.write_all(data).await?;
        file.sync_all().await
    }
    .await;
    drop(file);
    if written.is_err() {
        let _ = tokio::fs::remove_file(path).await;
    }
    written
}

pub async fn submit(
    dir: &Path,
    request: &Request,
    credentials: Option<(&str, &str)>,
) -> std::io::Result<()> {
    if !valid_module(&request.module) {
        return Err(std::io::Error::other("неверное имя модуля"));
    }
    let target = request_path(dir);
    if tokio::fs::try_exists(&target).await? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "заявка уже лежит",
        ));
    }
    let cred = credentials_path(dir, &request.id);
    if let Some((key, secret)) = credentials {
        write_private(&cred, format!("{key}\n{secret}\n").as_bytes()).await?;
    }
    let temp = dir
        .join("request")
        .join(format!(".request.{}.tmp", request.id));
    let result = async {
        write_private(&temp, request.line().as_bytes()).await?;
        if tokio::fs::try_exists(&target).await? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "заявка уже лежит",
            ));
        }
        tokio::fs::rename(&temp, &target).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
        let _ = tokio::fs::remove_file(&cred).await;
    }
    result
}

pub async fn withdraw(dir: &Path, request: &Uuid) {
    let target = request_path(dir);
    if read_small(&target).is_some_and(|text| text.contains(&request.to_string())) {
        let _ = tokio::fs::remove_file(&target).await;
    }
    let _ = tokio::fs::remove_file(credentials_path(dir, request)).await;
}

pub fn pending(dir: &Path, request: &str) -> bool {
    read_small(&request_path(dir)).is_some_and(|text| text.contains(request))
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StepFile {
    pub key: String,
    pub state: String,
    #[serde(default)]
    pub seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct JobFile {
    pub id: String,
    pub action: String,
    pub state: String,
    #[serde(default)]
    pub steps: Vec<StepFile>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub restored: Option<bool>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ContainerFile {
    pub state: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub restarts: Option<u64>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub previous: Option<String>,
    #[serde(default)]
    pub memory_bytes: Option<u64>,
    #[serde(default)]
    pub data_bytes: Option<u64>,
    #[serde(default)]
    pub checked_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StatusFile {
    #[serde(default)]
    pub job: Option<JobFile>,
    #[serde(default)]
    pub container: Option<ContainerFile>,
}

fn modules_dir(dir: &Path) -> PathBuf {
    dir.join("status").join("modules")
}

pub fn read_status(dir: &Path, module: &str) -> Option<StatusFile> {
    if !valid_module(module) {
        return None;
    }
    let text = read_small(&modules_dir(dir).join(format!("{module}.json")))?;
    serde_json::from_str(&text).ok()
}

pub fn read_logs(dir: &Path, module: &str) -> Option<(Vec<String>, Option<DateTime<Utc>>)> {
    if !valid_module(module) {
        return None;
    }
    let path = modules_dir(dir).join(format!("{module}.log"));
    let text = read_small(&path)?;
    let at = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .map(DateTime::<Utc>::from);
    let lines: Vec<String> = text
        .lines()
        .map(clean_line)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(LOG_LINES)
        .rev()
        .collect();
    Some((lines, at))
}

pub fn clean_line(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if c == '\t' {
            out.push(' ');
        } else if !c.is_control() {
            out.push(c);
        }
        if out.chars().count() >= LOG_LINE_MAX {
            break;
        }
    }
    out
}

fn time(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
        .map(|t| t.with_timezone(&Utc))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepView {
    pub key: String,
    pub state: String,
    pub seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobView {
    pub id: String,
    pub action: String,
    pub state: String,
    pub steps: Vec<StepView>,
    pub error: Option<String>,
    pub restored: Option<bool>,
    pub requested_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl JobView {
    pub fn active(&self) -> bool {
        matches!(self.state.as_str(), "waiting" | "running")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Marker {
    pub id: String,
    pub action: String,
    pub requested_at: DateTime<Utc>,
    #[serde(default)]
    pub admin_id: Option<i64>,
}

pub fn error_text(code: &str, message: Option<&str>) -> String {
    let text = match code {
        "invalid_request" => "Помощник не принял заявку",
        "expired" => {
            "Помощник не взял заявку за 10 минут: проверьте командой sudo stealthnet status"
        }
        "busy" => "Помощник был занят другой заявкой, повторите через минуту",
        "blocked" | "modules_off" => {
            "Установка модулей на сервере запрещена командой stealthnet modules off"
        }
        "package_missing" => "Архив модуля не найден на сервере: загрузите его заново",
        "package_invalid" => "Архив не прошёл проверку на сервере",
        "wrong_arch" => "Образ собран не для процессора этого сервера",
        "load_failed" => "Docker не загрузил образ модуля",
        "docker_unavailable" => "Docker на сервере не отвечает",
        "no_space" => "На сервере не хватает места",
        "start_failed" => "Контейнер модуля не запустился",
        "unhealthy" => "Модуль не ответил на проверку работы за 90 секунд",
        "not_installed" => "Модуля нет на сервере: загрузите архив заново",
        "interrupted" => "Заявка прервалась",
        _ => "Внутренняя ошибка помощника",
    };
    match message.map(str::trim).filter(|m| !m.is_empty()) {
        Some(message) => {
            let message: String = clean_line(message).chars().take(MESSAGE_MAX).collect();
            format!("{text}: {message}")
        }
        None => text.to_string(),
    }
}

fn default_steps(action: &str) -> Vec<StepView> {
    Action::parse(action)
        .map(|a| a.steps())
        .unwrap_or(&[])
        .iter()
        .map(|key| StepView {
            key: (*key).to_string(),
            state: "pending".into(),
            seconds: None,
        })
        .collect()
}

pub fn job_view(
    marker: Option<&Marker>,
    status: Option<&StatusFile>,
    waiting: bool,
    now: DateTime<Utc>,
) -> Option<JobView> {
    let file = status.and_then(|s| s.job.as_ref());
    let from_file = |job: &JobFile, requested_at: Option<DateTime<Utc>>| JobView {
        id: job.id.clone(),
        action: job.action.clone(),
        state: job.state.clone(),
        steps: if job.steps.is_empty() {
            default_steps(&job.action)
        } else {
            job.steps
                .iter()
                .map(|s| StepView {
                    key: s.key.clone(),
                    state: s.state.clone(),
                    seconds: s.seconds,
                })
                .collect()
        },
        error: job
            .error
            .as_deref()
            .map(|code| error_text(code, job.message.as_deref())),
        restored: job.restored,
        requested_at: requested_at.or_else(|| time(job.started_at.as_deref())),
        finished_at: time(job.finished_at.as_deref()),
    };
    let Some(marker) = marker else {
        return file.map(|job| from_file(job, None));
    };
    if let Some(job) = file.filter(|job| job.id == marker.id) {
        return Some(from_file(job, Some(marker.requested_at)));
    }
    let expired =
        !waiting && now - marker.requested_at > Duration::seconds(REQUEST_EXPIRES_SECONDS);
    let (state, error) = if expired {
        ("failed", Some(error_text("expired", None)))
    } else {
        ("waiting", None)
    };
    Some(JobView {
        id: marker.id.clone(),
        action: marker.action.clone(),
        state: state.into(),
        steps: default_steps(&marker.action),
        error,
        restored: None,
        requested_at: Some(marker.requested_at),
        finished_at: None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainerView {
    pub state: String,
    pub started_at: Option<DateTime<Utc>>,
    pub restarts: Option<u64>,
    pub image: Option<String>,
    pub memory_bytes: Option<u64>,
    pub data_bytes: Option<u64>,
    pub checked_at: Option<DateTime<Utc>>,
}

pub fn container_view(status: Option<&StatusFile>) -> Option<ContainerView> {
    let container = status?.container.as_ref()?;
    Some(ContainerView {
        state: container.state.clone(),
        started_at: time(container.started_at.as_deref()),
        restarts: container.restarts,
        image: container.image.clone(),
        memory_bytes: container.memory_bytes,
        data_bytes: container.data_bytes,
        checked_at: time(container.checked_at.as_deref()),
    })
}

pub fn host(config: Option<&UpdateConfig>, module: &str) -> String {
    config
        .and_then(|c| c.modules_host.clone())
        .unwrap_or_else(|| format!("{HOST_PREFIX}{module}"))
}

pub fn url(config: Option<&UpdateConfig>, module: &str, port: u16, path: &str) -> String {
    format!("http://{}:{port}{path}", host(config, module))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn заявки_помощнику_по_строгому_шаблону() {
        let at = DateTime::parse_from_rfc3339("2026-11-24T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let id = Uuid::parse_str("6b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10").unwrap();
        let package = Uuid::parse_str("1b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e11").unwrap();
        let mut request = Request {
            id,
            action: Action::ModuleInstall,
            module: "nodes".into(),
            package: Some(package),
            erase: None,
            admin_id: 1,
            at,
        };
        assert_eq!(
            request.line(),
            "{\"id\":\"6b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10\",\"action\":\"module-install\",\"module\":\"nodes\",\"package\":\"1b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e11\",\"requested_at\":\"2026-11-24T10:00:00Z\",\"admin_id\":1}\n"
        );
        request.action = Action::ModuleRemove;
        request.package = None;
        request.erase = Some(true);
        assert_eq!(
            request.line(),
            "{\"id\":\"6b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10\",\"action\":\"module-remove\",\"module\":\"nodes\",\"data\":\"erase\",\"requested_at\":\"2026-11-24T10:00:00Z\",\"admin_id\":1}\n"
        );
        request.action = Action::ModuleRestart;
        request.erase = None;
        assert_eq!(
            request.line(),
            "{\"id\":\"6b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10\",\"action\":\"module-restart\",\"module\":\"nodes\",\"requested_at\":\"2026-11-24T10:00:00Z\",\"admin_id\":1}\n"
        );
    }

    #[test]
    fn строки_журнала_без_управляющих_символов() {
        assert_eq!(
            clean_line("\u{1b}[31mошибка\u{1b}[0m\tдальше\u{7}"),
            "ошибка дальше"
        );
        assert_eq!(clean_line(&"я".repeat(900)).chars().count(), LOG_LINE_MAX);
    }

    #[test]
    fn состояние_заявки_из_файла_помощника() {
        let now = Utc::now();
        let marker = Marker {
            id: "a".into(),
            action: "module-install".into(),
            requested_at: now - Duration::seconds(5),
            admin_id: Some(1),
        };
        let waiting = job_view(Some(&marker), None, true, now).unwrap();
        assert_eq!(waiting.state, "waiting");
        assert_eq!(waiting.steps.len(), 4);
        let status: StatusFile = serde_json::from_str(
            r#"{"job":{"id":"a","action":"module-install","state":"failed","steps":[{"key":"check","state":"failed","seconds":1}],"error":"package_invalid","message":"образ\u001b[0m не тот"}}"#,
        )
        .unwrap();
        let failed = job_view(Some(&marker), Some(&status), false, now).unwrap();
        assert_eq!(failed.state, "failed");
        assert_eq!(
            failed.error.as_deref(),
            Some("Архив не прошёл проверку на сервере: образ не тот")
        );
        let old = Marker {
            requested_at: now - Duration::seconds(REQUEST_EXPIRES_SECONDS + 1),
            ..marker.clone()
        };
        let expired = job_view(Some(&old), None, false, now).unwrap();
        assert_eq!(expired.state, "failed");
        assert!(expired.error.unwrap().contains("не взял заявку"));
        let still = job_view(Some(&old), None, true, now).unwrap();
        assert_eq!(still.state, "waiting");
    }
}
