use crate::Actor;
use crate::auth::RequestMeta;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rand::Rng;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, HeaderMap, WWW_AUTHENTICATE};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sn_domain::access::AdminRole;
use sn_domain::ids::AdminId;
use sn_store::{AuditEntry, Db};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const CHECK_KEY: &str = "system.updates.check";
pub const ENABLED_KEY: &str = "system.updates.check_enabled";
pub const REQUEST_KEY: &str = "system.updates.request";
pub const REPORTED_KEY: &str = "system.updates.reported";

pub const DEFAULT_REGISTRY: &str = "https://ghcr.io";
pub const DEFAULT_REPOSITORY: &str = "stealthnet-app/stealthnet-rw-system";
pub const DEFAULT_TAG: &str = "main";
pub const SOURCE_URL: &str = "https://github.com/STEALTHNET-APP/STEALTHNET-RW-SYSTEM";

pub const HELPER_MISSING: &str = "system.helper_missing";
pub const UPDATE_IN_PROGRESS: &str = "system.update_in_progress";
pub const UPDATE_STALE: &str = "system.update_stale";
pub const UPDATE_STARTED: &str = "system.update_started";
pub const UPDATE_RATE_LIMITED: &str = "system.update_rate_limited";
pub const CHECK_RATE_LIMITED: &str = "system.check_rate_limited";
pub const CHECK_DISABLED: &str = "system.check_disabled";

const MANIFEST_TYPES: &str = "application/vnd.oci.image.index.v1+json, \
application/vnd.docker.distribution.manifest.list.v2+json, \
application/vnd.oci.image.manifest.v1+json, \
application/vnd.docker.distribution.manifest.v2+json";
const MANIFEST_LIMIT: usize = 64 * 1024;
const CONFIG_LIMIT: usize = 512 * 1024;
const TOKEN_LIMIT: usize = 16 * 1024;
const FILE_LIMIT: usize = 64 * 1024;
const HISTORY_SHOWN: usize = 5;
const SILENT_SECONDS: i64 = 30;
const LOST_SECONDS: i64 = 600;
const REQUEST_GAP_SECONDS: i64 = 30;
const CHECK_GAP_SECONDS: i64 = 60;
const RECENT_HOURS: i64 = 24;
const CHECK_EVERY_HOURS: i64 = 6;
const STALE_HOURS: i64 = 20;
const USER_AGENT: &str = "stealthnet-update-check";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildInfo {
    pub version: String,
    pub label: Option<String>,
    pub commit: Option<String>,
    pub built_at: Option<DateTime<Utc>>,
    pub arch: String,
}

impl BuildInfo {
    pub fn new(commit: Option<&str>, built_at: Option<&str>) -> Self {
        BuildInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            label: None,
            commit: commit.and_then(clean_commit),
            built_at: built_at.and_then(parse_time),
            arch: host_arch().to_string(),
        }
    }

    pub fn with_label(mut self, label: Option<&str>) -> Self {
        self.label = label.and_then(clean_label);
        self
    }
}

pub fn clean_label(value: &str) -> Option<String> {
    let value = value.trim();
    let parts: Vec<&str> = value.split('.').collect();
    (parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit())))
    .then(|| value.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateConfig {
    pub build: BuildInfo,
    pub dir: Option<PathBuf>,
    pub registry: String,
    pub repository: String,
    pub tag: String,
    pub checks_allowed: bool,
    pub helper_why: Option<String>,
    pub modules_host: Option<String>,
}

impl UpdateConfig {
    pub fn new(build: BuildInfo) -> Self {
        UpdateConfig {
            build,
            dir: None,
            registry: DEFAULT_REGISTRY.to_string(),
            repository: DEFAULT_REPOSITORY.to_string(),
            tag: DEFAULT_TAG.to_string(),
            checks_allowed: true,
            helper_why: None,
            modules_host: None,
        }
    }
}

pub fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

pub fn is_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn clean_commit(value: &str) -> Option<String> {
    let value = value.trim();
    is_commit(value).then(|| value.to_ascii_lowercase())
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn valid_digest(value: &&str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedBuild {
    #[serde(default)]
    pub label: Option<String>,
    pub commit: Option<String>,
    pub built_at: DateTime<Utc>,
    pub arch: String,
    #[serde(default)]
    pub digest: Option<String>,
}

pub fn is_newer(current: &BuildInfo, latest: &PublishedBuild) -> bool {
    let Some(commit) = latest.commit.as_deref() else {
        return false;
    };
    if current.commit.as_deref() == Some(commit) {
        return false;
    }
    current.built_at.is_some_and(|at| latest.built_at > at)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckFailure {
    Unreachable(String),
    Invalid(String),
    NoBuild,
}

impl std::fmt::Display for CheckFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckFailure::Unreachable(why) => write!(f, "реестр сборок не ответил: {why}"),
            CheckFailure::Invalid(why) => write!(f, "реестр сборок ответил непонятно: {why}"),
            CheckFailure::NoBuild => write!(f, "готовой сборки для этого процессора нет"),
        }
    }
}

fn unreachable(err: reqwest::Error) -> CheckFailure {
    CheckFailure::Unreachable(err.without_url().to_string())
}

fn parse_json(body: &[u8]) -> Result<Value, CheckFailure> {
    serde_json::from_slice(body).map_err(|_| CheckFailure::Invalid("не JSON".into()))
}

fn parse_challenge(header: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    let rest = header.trim();
    let rest = match rest.get(..7) {
        Some(prefix) if prefix.eq_ignore_ascii_case("bearer ") => &rest[7..],
        _ => return params,
    };
    let mut chars = rest.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| *c == ',' || c.is_whitespace()) {
            chars.next();
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' {
                break;
            }
            key.push(c);
            chars.next();
        }
        if chars.next().is_none() {
            break;
        }
        let mut value = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => {
                        if let Some(next) = chars.next() {
                            value.push(next);
                        }
                    }
                    other => value.push(other),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ',' {
                    break;
                }
                value.push(c);
                chars.next();
            }
        }
        params.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    params
}

async fn read_limited(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, CheckFailure> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(unreachable)? {
        if body.len() + chunk.len() > limit {
            return Err(CheckFailure::Invalid("слишком большой ответ".into()));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

struct Registry<'a> {
    http: &'a reqwest::Client,
    base: String,
    repository: &'a str,
    token: Option<String>,
}

impl Registry<'_> {
    async fn get(
        &mut self,
        path: &str,
        accept: Option<&str>,
        limit: usize,
    ) -> Result<(HeaderMap, Vec<u8>), CheckFailure> {
        let url = format!("{}/v2/{}/{}", self.base, self.repository, path);
        let mut authorized = false;
        loop {
            let mut request = self.http.get(&url);
            if let Some(accept) = accept {
                request = request.header(ACCEPT, accept);
            }
            if let Some(token) = &self.token {
                request = request.bearer_auth(token);
            }
            let response = request.send().await.map_err(unreachable)?;
            let status = response.status();
            if status == StatusCode::UNAUTHORIZED && !authorized {
                let challenge = response
                    .headers()
                    .get(WWW_AUTHENTICATE)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
                    .unwrap_or_default();
                self.token = Some(self.authorize(&challenge).await?);
                authorized = true;
                continue;
            }
            if status == StatusCode::NOT_FOUND {
                return Err(CheckFailure::NoBuild);
            }
            if !status.is_success() {
                return Err(CheckFailure::Unreachable(format!(
                    "ответ {}",
                    status.as_u16()
                )));
            }
            let headers = response.headers().clone();
            return Ok((headers, read_limited(response, limit).await?));
        }
    }

    async fn authorize(&self, challenge: &str) -> Result<String, CheckFailure> {
        let params = parse_challenge(challenge);
        let realm = params
            .get("realm")
            .filter(|r| r.starts_with("https://") || r.starts_with("http://"))
            .ok_or_else(|| CheckFailure::Invalid("реестр не сказал, где взять вход".into()))?;
        let mut query = Vec::new();
        if let Some(service) = params.get("service") {
            query.push(("service", service.clone()));
        }
        let scope = params
            .get("scope")
            .cloned()
            .unwrap_or_else(|| format!("repository:{}:pull", self.repository));
        query.push(("scope", scope));
        let response = self
            .http
            .get(realm)
            .query(&query)
            .send()
            .await
            .map_err(unreachable)?;
        if !response.status().is_success() {
            return Err(CheckFailure::Unreachable(format!(
                "вход в реестр: ответ {}",
                response.status().as_u16()
            )));
        }
        let body = parse_json(&read_limited(response, TOKEN_LIMIT).await?)?;
        body.get("token")
            .or_else(|| body.get("access_token"))
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .ok_or_else(|| CheckFailure::Invalid("реестр не выдал вход".into()))
    }
}

pub async fn fetch_latest(
    http: &reqwest::Client,
    config: &UpdateConfig,
) -> Result<PublishedBuild, CheckFailure> {
    let arch = config.build.arch.as_str();
    let mut registry = Registry {
        http,
        base: config.registry.trim_end_matches('/').to_string(),
        repository: &config.repository,
        token: None,
    };
    let (headers, body) = registry
        .get(
            &format!("manifests/{}", config.tag),
            Some(MANIFEST_TYPES),
            MANIFEST_LIMIT,
        )
        .await?;
    let mut manifest = parse_json(&body)?;
    let mut digest = headers
        .get("docker-content-digest")
        .and_then(|v| v.to_str().ok())
        .filter(valid_digest)
        .map(str::to_string);
    if let Some(list) = manifest.get("manifests").and_then(Value::as_array) {
        let child = list
            .iter()
            .find(|m| {
                m.pointer("/platform/os").and_then(Value::as_str) == Some("linux")
                    && m.pointer("/platform/architecture").and_then(Value::as_str) == Some(arch)
            })
            .ok_or(CheckFailure::NoBuild)?
            .get("digest")
            .and_then(Value::as_str)
            .filter(valid_digest)
            .ok_or_else(|| CheckFailure::Invalid("нет отпечатка сборки".into()))?
            .to_string();
        let (_, body) = registry
            .get(
                &format!("manifests/{child}"),
                Some(MANIFEST_TYPES),
                MANIFEST_LIMIT,
            )
            .await?;
        manifest = parse_json(&body)?;
        digest = Some(child);
    }
    let config_digest = manifest
        .pointer("/config/digest")
        .and_then(Value::as_str)
        .filter(valid_digest)
        .ok_or_else(|| CheckFailure::Invalid("нет описания сборки".into()))?
        .to_string();
    let (_, body) = registry
        .get(&format!("blobs/{config_digest}"), None, CONFIG_LIMIT)
        .await?;
    let image = parse_json(&body)?;
    let image_arch = image
        .get("architecture")
        .and_then(Value::as_str)
        .unwrap_or(arch);
    if image_arch != arch {
        return Err(CheckFailure::NoBuild);
    }
    let label = |name: &str| {
        image
            .pointer("/config/Labels")
            .and_then(|labels| labels.get(name))
            .and_then(Value::as_str)
    };
    let built_at = label("org.opencontainers.image.created")
        .and_then(parse_time)
        .or_else(|| {
            image
                .get("created")
                .and_then(Value::as_str)
                .and_then(parse_time)
        })
        .ok_or_else(|| CheckFailure::Invalid("нет даты сборки".into()))?;
    Ok(PublishedBuild {
        label: label("org.opencontainers.image.version").and_then(clean_label),
        commit: label("org.opencontainers.image.revision").and_then(clean_commit),
        built_at,
        arch: arch.to_string(),
        digest,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    #[default]
    Never,
    Ok,
    Unreachable,
    Invalid,
    NoBuild,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckState {
    #[serde(default)]
    pub state: CheckOutcome,
    #[serde(default)]
    pub last_checked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_ok_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub latest: Option<PublishedBuild>,
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub checked_build: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Jitter {
    soon: Duration,
    spread: Duration,
    stale: Duration,
}

fn build_key(build: &BuildInfo) -> String {
    build
        .commit
        .clone()
        .or_else(|| build.label.clone())
        .unwrap_or_else(|| build.version.clone())
}

fn check_due(state: &CheckState, build: &str, now: DateTime<Utc>, jitter: Jitter) -> DateTime<Utc> {
    match (state.last_checked_at, state.last_ok_at) {
        (None, _) => now + jitter.soon,
        (Some(_), _) if state.checked_build.as_deref() != Some(build) => now + jitter.soon,
        (Some(last), _) if state.failures > 0 => {
            let hours = 1i64 << state.failures.saturating_sub(1).min(5);
            last + Duration::hours(hours.min(CHECK_EVERY_HOURS))
        }
        (Some(_), Some(ok)) if now - ok > Duration::hours(STALE_HOURS) => now + jitter.stale,
        (Some(last), _) => last + Duration::hours(CHECK_EVERY_HOURS) + jitter.spread,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MarkerState {
    #[default]
    Requested,
    Canceled,
    Silent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RequestMarker {
    id: Uuid,
    target: String,
    #[serde(default)]
    from: Option<String>,
    admin_id: i64,
    requested_at: DateTime<Utc>,
    #[serde(default)]
    state: MarkerState,
    #[serde(default)]
    from_label: Option<String>,
    #[serde(default)]
    to_label: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct HelperFile {
    #[serde(default)]
    pub(crate) panel_updates: bool,
    #[serde(default)]
    pub(crate) modules: Option<bool>,
    #[serde(default)]
    blocked: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepView {
    pub key: String,
    pub state: String,
    #[serde(default)]
    pub seconds: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RunFile {
    #[serde(default)]
    id: String,
    #[serde(default)]
    origin: String,
    #[serde(default)]
    admin_id: Option<i64>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    from_version: Option<String>,
    #[serde(default)]
    to_version: Option<String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    steps: Vec<StepView>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    finished_at: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    seconds: Option<u64>,
    #[serde(default)]
    downtime: Option<u64>,
    #[serde(default)]
    backup: Option<String>,
}

const ACTIVE: [&str; 3] = ["requested", "waiting", "running"];
const STEP_KEYS: [&str; 6] = ["check", "download", "backup", "migrate", "start", "verify"];
const STEP_STATES: [&str; 4] = ["pending", "running", "done", "failed"];

fn token(value: &str, max: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.'))
    .then(|| value.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CurrentView {
    pub version: String,
    pub label: Option<String>,
    pub commit: Option<String>,
    pub built_at: Option<DateTime<Utc>>,
    pub arch: String,
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComponentsView {
    pub remnawave: &'static str,
    pub postgres: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckView {
    pub enabled: bool,
    pub allowed: bool,
    pub state: CheckOutcome,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_ok_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LatestView {
    pub label: Option<String>,
    pub commit: String,
    pub built_at: DateTime<Utc>,
    pub source_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HelperView {
    pub installed: bool,
    pub panel_updates: bool,
    pub blocked: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateView {
    pub id: String,
    pub origin: String,
    pub state: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub from_label: Option<String>,
    pub to_label: Option<String>,
    pub steps: Vec<StepView>,
    pub requested_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub downtime: Option<u64>,
    pub backup: Option<String>,
    pub by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistoryView {
    pub id: String,
    pub origin: String,
    pub result: String,
    pub error: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub from_label: Option<String>,
    pub to_label: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub seconds: Option<u64>,
    pub by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SystemView {
    pub current: CurrentView,
    pub components: ComponentsView,
    pub check: CheckView,
    pub latest: Option<LatestView>,
    pub helper: HelperView,
    pub update: Option<UpdateView>,
    pub history: Vec<HistoryView>,
    pub can_update: bool,
    pub owner: bool,
    pub totp_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SummaryView {
    pub version: String,
    pub label: Option<String>,
    pub commit: Option<String>,
    pub built_at: Option<DateTime<Utc>>,
    pub update: Option<LatestView>,
}

fn source_url(commit: &str) -> String {
    format!("{SOURCE_URL}/tree/{commit}")
}

pub(crate) fn read_small(path: &Path) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(FILE_LIMIT as u64 + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() <= FILE_LIMIT).then_some(text)
}

pub(crate) fn read_helper(dir: &Path) -> Option<HelperFile> {
    serde_json::from_str(&read_small(&dir.join("status").join("helper.json"))?).ok()
}

fn clean_run(mut run: RunFile) -> Option<RunFile> {
    run.id = token(&run.id, 64)?;
    run.origin = token(&run.origin, 16).unwrap_or_else(|| "command".into());
    run.state = token(&run.state, 16)?;
    run.from = run.from.as_deref().and_then(clean_commit);
    run.to = run.to.as_deref().and_then(clean_commit);
    run.from_version = run.from_version.as_deref().and_then(clean_label);
    run.to_version = run.to_version.as_deref().and_then(clean_label);
    run.error = run.error.as_deref().and_then(|e| token(e, 40));
    run.backup = run.backup.as_deref().and_then(|b| token(b, 80));
    run.steps
        .retain(|s| STEP_KEYS.contains(&s.key.as_str()) && STEP_STATES.contains(&s.state.as_str()));
    Some(run)
}

fn read_run(dir: &Path) -> Option<RunFile> {
    clean_run(serde_json::from_str(&read_small(&dir.join("status").join("update.json"))?).ok()?)
}

fn read_history(dir: &Path) -> Vec<RunFile> {
    let Some(text) = read_small(&dir.join("status").join("history.jsonl")) else {
        return Vec::new();
    };
    let mut runs: Vec<RunFile> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter_map(clean_run)
        .collect();
    runs.reverse();
    runs.truncate(HISTORY_SHOWN);
    runs
}

fn request_path(dir: &Path) -> PathBuf {
    dir.join("request").join("request.json")
}

fn time(value: Option<&str>) -> Option<DateTime<Utc>> {
    value.and_then(parse_time)
}

pub struct Updates<'a> {
    services: &'a Services,
}

impl<'a> Updates<'a> {
    pub fn new(services: &'a Services) -> Self {
        Updates { services }
    }

    fn config(&self) -> UpdateConfig {
        self.services
            .update_config()
            .map(|c| (*c).clone())
            .unwrap_or_else(|| UpdateConfig {
                checks_allowed: false,
                ..UpdateConfig::new(BuildInfo::new(None, None))
            })
    }

    async fn check_state(&self) -> ServiceResult<CheckState> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .bot_state()
            .get(CHECK_KEY)
            .await?
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default())
    }

    async fn checks_enabled(&self) -> ServiceResult<bool> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .bot_state()
            .get(ENABLED_KEY)
            .await?
            .and_then(|v| v.as_bool())
            .unwrap_or(true))
    }

    async fn marker(&self) -> ServiceResult<Option<RequestMarker>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .bot_state()
            .get(REQUEST_KEY)
            .await?
            .and_then(|v| serde_json::from_value(v).ok()))
    }

    async fn save(&self, key: &str, value: &impl Serialize) -> ServiceResult<()> {
        let value =
            serde_json::to_value(value).map_err(|e| ServiceError::Internal(e.to_string()))?;
        let mut conn = self.services.store().conn().await?;
        conn.bot_state().set(key, &value).await?;
        Ok(())
    }

    async fn audit(&self, entry: AuditEntry) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        conn.audit().record(&entry).await?;
        Ok(())
    }

    fn entry(actor: Actor, action: &str, details: Value, meta: Option<&RequestMeta>) -> AuditEntry {
        let mut entry = actor.audit(action, "system", None, details);
        entry.ip = meta.and_then(|m| m.ip.clone());
        entry
    }

    async fn emails(&self, ids: impl Iterator<Item = i64>) -> ServiceResult<HashMap<i64, String>> {
        let mut map = HashMap::new();
        let mut conn = self.services.store().conn().await?;
        for id in ids {
            if map.contains_key(&id) {
                continue;
            }
            if let Ok(admin) = conn.admins().get(AdminId(id)).await {
                map.insert(id, admin.email);
            }
        }
        Ok(map)
    }

    async fn postgres_version(&self) -> Option<String> {
        let mut conn = self.services.store().conn().await.ok()?;
        let version: String = sqlx::query_scalar("SHOW server_version")
            .fetch_one(conn.conn())
            .await
            .ok()?;
        version.split_whitespace().next().map(str::to_string)
    }

    pub async fn summary(&self, role: AdminRole) -> ServiceResult<SummaryView> {
        let config = self.config();
        let mut update = None;
        if role == AdminRole::Owner {
            let check = self.check_state().await?;
            let blocked = config
                .dir
                .as_deref()
                .and_then(read_helper)
                .and_then(|h| h.blocked)
                .and_then(|b| clean_commit(&b));
            update = check
                .latest
                .filter(|b| is_newer(&config.build, b))
                .filter(|b| b.commit != blocked)
                .and_then(|b| {
                    b.commit.map(|commit| LatestView {
                        source_url: source_url(&commit),
                        label: b.label,
                        commit,
                        built_at: b.built_at,
                    })
                });
        }
        Ok(SummaryView {
            version: config.build.version,
            label: config.build.label,
            commit: config.build.commit,
            built_at: config.build.built_at,
            update,
        })
    }

    pub async fn overview(&self, admin: AdminId, role: AdminRole) -> ServiceResult<SystemView> {
        let config = self.config();
        let owner = role == AdminRole::Owner;
        let check = self.check_state().await?;
        let enabled = self.checks_enabled().await?;
        let helper_file = config.dir.as_deref().and_then(read_helper);
        let helper = HelperView {
            installed: helper_file.is_some(),
            panel_updates: helper_file.as_ref().is_some_and(|h| h.panel_updates),
            blocked: helper_file
                .as_ref()
                .and_then(|h| h.blocked.as_deref())
                .and_then(clean_commit),
        };
        let update = self.current_update(&config).await?;
        let history = config.dir.as_deref().map(read_history).unwrap_or_default();
        let ids = update
            .iter()
            .filter_map(|(_, admin)| *admin)
            .chain(history.iter().filter_map(|run| run.admin_id));
        let emails = self.emails(ids).await?;
        let by = |origin: &str, admin: Option<i64>| match origin {
            "panel" => admin.and_then(|id| emails.get(&id).cloned()),
            _ => None,
        };
        let update = update.map(|(mut view, admin)| {
            view.by = by(&view.origin, admin);
            view
        });
        let history = history
            .into_iter()
            .map(|run| HistoryView {
                by: by(&run.origin, run.admin_id),
                id: run.id,
                origin: run.origin,
                result: run.state,
                error: run.error,
                from: run.from,
                to: run.to,
                from_label: run.from_version,
                to_label: run.to_version,
                started_at: time(run.started_at.as_deref()),
                finished_at: time(run.finished_at.as_deref()),
                seconds: run.seconds,
            })
            .collect();
        let latest = check
            .latest
            .as_ref()
            .filter(|b| is_newer(&config.build, b))
            .and_then(|b| {
                b.commit.as_ref().map(|commit| LatestView {
                    label: b.label.clone(),
                    commit: commit.clone(),
                    built_at: b.built_at,
                    source_url: source_url(commit),
                })
            });
        let active = update
            .as_ref()
            .is_some_and(|u| ACTIVE.contains(&u.state.as_str()));
        let can_update = owner
            && helper.panel_updates
            && !active
            && latest
                .as_ref()
                .is_some_and(|l| helper.blocked.as_deref() != Some(l.commit.as_str()));
        let totp_required = if owner {
            let mut conn = self.services.store().conn().await?;
            conn.admins().get(admin).await?.totp.is_some()
        } else {
            false
        };
        Ok(SystemView {
            current: CurrentView {
                source_url: config.build.commit.as_deref().map(source_url),
                version: config.build.version.clone(),
                label: config.build.label.clone(),
                commit: config.build.commit.clone(),
                built_at: config.build.built_at,
                arch: config.build.arch.clone(),
            },
            components: ComponentsView {
                remnawave: sn_remnawave::SUPPORTED_VERSION,
                postgres: self.postgres_version().await,
            },
            check: CheckView {
                enabled,
                allowed: config.checks_allowed,
                state: check.state,
                last_checked_at: check.last_checked_at,
                last_ok_at: check.last_ok_at,
            },
            latest: if owner { latest } else { None },
            helper,
            update: if owner { update } else { None },
            history,
            can_update,
            owner,
            totp_required,
        })
    }

    async fn current_update(
        &self,
        config: &UpdateConfig,
    ) -> ServiceResult<Option<(UpdateView, Option<i64>)>> {
        let Some(dir) = config.dir.as_deref() else {
            return Ok(None);
        };
        let now = self.services.now();
        let run = read_run(dir);
        if let Some(run) = &run {
            self.report(run).await?;
        }
        let marker = self.marker().await?;
        if let Some(mut marker) = marker {
            let id = marker.id.to_string();
            if let Some(run) = run.as_ref().filter(|r| r.id == id) {
                return Ok(Some((
                    Self::run_view(run, Some(marker.requested_at)),
                    Some(marker.admin_id),
                )));
            }
            let age = now - marker.requested_at;
            if marker.state == MarkerState::Requested {
                let waiting = request_path(dir).exists();
                let limit = if waiting {
                    SILENT_SECONDS
                } else {
                    LOST_SECONDS
                };
                if age > Duration::seconds(limit) {
                    let removed = remove_request(dir).await;
                    if removed || !waiting {
                        marker.state = MarkerState::Silent;
                        self.save(REQUEST_KEY, &marker).await?;
                        self.audit(Self::entry(
                            Actor::System,
                            "system.update_canceled",
                            serde_json::json!({ "id": id, "reason": "helper_silent" }),
                            None,
                        ))
                        .await?;
                    }
                } else {
                    return Ok(Some((
                        Self::marker_view(&marker, "requested", None),
                        Some(marker.admin_id),
                    )));
                }
            }
            if marker.state == MarkerState::Silent && age < Duration::hours(RECENT_HOURS) {
                let newer_run = run
                    .as_ref()
                    .and_then(|r| time(r.started_at.as_deref()))
                    .is_some_and(|at| at > marker.requested_at);
                if !newer_run {
                    return Ok(Some((
                        Self::marker_view(&marker, "failed", Some("helper_silent")),
                        Some(marker.admin_id),
                    )));
                }
            }
        }
        let Some(run) = run else {
            return Ok(None);
        };
        let recent = ACTIVE.contains(&run.state.as_str())
            || time(run.finished_at.as_deref())
                .or_else(|| time(run.started_at.as_deref()))
                .is_some_and(|at| now - at < Duration::hours(RECENT_HOURS));
        Ok(recent.then(|| (Self::run_view(&run, None), run.admin_id)))
    }

    fn marker_view(marker: &RequestMarker, state: &str, error: Option<&str>) -> UpdateView {
        UpdateView {
            id: marker.id.to_string(),
            origin: "panel".into(),
            state: state.into(),
            from: marker.from.clone(),
            to: Some(marker.target.clone()),
            from_label: marker.from_label.clone(),
            to_label: marker.to_label.clone(),
            steps: STEP_KEYS
                .iter()
                .map(|key| StepView {
                    key: (*key).into(),
                    state: "pending".into(),
                    seconds: None,
                })
                .collect(),
            requested_at: Some(marker.requested_at),
            started_at: None,
            finished_at: None,
            error: error.map(str::to_string),
            downtime: None,
            backup: None,
            by: None,
        }
    }

    fn run_view(run: &RunFile, requested_at: Option<DateTime<Utc>>) -> UpdateView {
        UpdateView {
            id: run.id.clone(),
            origin: run.origin.clone(),
            state: run.state.clone(),
            from: run.from.clone(),
            to: run.to.clone(),
            from_label: run.from_version.clone(),
            to_label: run.to_version.clone(),
            steps: run.steps.clone(),
            requested_at,
            started_at: time(run.started_at.as_deref()),
            finished_at: time(run.finished_at.as_deref()),
            error: run.error.clone(),
            downtime: run.downtime,
            backup: run.backup.clone(),
            by: None,
        }
    }

    async fn report(&self, run: &RunFile) -> ServiceResult<()> {
        let action = match run.state.as_str() {
            "done" => "system.update_finished",
            "failed" => "system.update_failed",
            "rolled_back" => "system.update_rolled_back",
            "attention" => "system.update_attention",
            _ => return Ok(()),
        };
        let mut tx = self.services.store().begin().await?;
        let reported = tx.bot_state().get(REPORTED_KEY).await?;
        if reported.as_ref().and_then(Value::as_str) == Some(run.id.as_str()) {
            return Ok(());
        }
        tx.bot_state()
            .set(REPORTED_KEY, &Value::String(run.id.clone()))
            .await?;
        tx.audit()
            .record(&Self::entry(
                Actor::System,
                action,
                serde_json::json!({
                    "id": run.id,
                    "origin": run.origin,
                    "admin_id": run.admin_id,
                    "from": run.from,
                    "to": run.to,
                    "error": run.error,
                    "seconds": run.seconds,
                    "downtime": run.downtime,
                    "backup": run.backup,
                }),
                None,
            ))
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn run_check(&self, config: &UpdateConfig) -> ServiceResult<CheckState> {
        let proxy = self
            .services
            .egress()
            .proxy_for(sn_egress::Route::Updates, None);
        let http = sn_egress::build(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .connect_timeout(std::time::Duration::from_secs(5))
                .user_agent(USER_AGENT),
            proxy.as_deref(),
        );
        let result = fetch_latest(&http, config).await;
        let now = self.services.now();
        let mut state = self.check_state().await?;
        state.last_checked_at = Some(now);
        state.checked_build = Some(build_key(&config.build));
        match result {
            Ok(build) => {
                state.state = CheckOutcome::Ok;
                state.last_ok_at = Some(now);
                state.latest = Some(build);
                state.failures = 0;
            }
            Err(CheckFailure::NoBuild) => {
                state.state = CheckOutcome::NoBuild;
                state.last_ok_at = Some(now);
                state.latest = None;
                state.failures = 0;
            }
            Err(failure) => {
                tracing::warn!(error = %failure, "проверка новой версии не удалась");
                state.state = match failure {
                    CheckFailure::Invalid(_) => CheckOutcome::Invalid,
                    _ => CheckOutcome::Unreachable,
                };
                state.failures = state.failures.saturating_add(1);
            }
        }
        self.save(CHECK_KEY, &state).await?;
        Ok(state)
    }

    pub async fn check_now(&self, admin: AdminId, role: AdminRole) -> ServiceResult<SystemView> {
        if role != AdminRole::Owner {
            return Err(ServiceError::Forbidden);
        }
        let config = self.config();
        if !config.checks_allowed {
            return Err(ServiceError::rejected(
                CHECK_DISABLED,
                "Проверка новых версий выключена на сервере переменной SN_UPDATE_CHECK",
            ));
        }
        let state = self.check_state().await?;
        if state
            .last_checked_at
            .is_some_and(|at| self.services.now() - at < Duration::seconds(CHECK_GAP_SECONDS))
        {
            return Err(ServiceError::rejected(
                CHECK_RATE_LIMITED,
                "Проверять можно раз в минуту",
            ));
        }
        self.run_check(&config).await?;
        self.overview(admin, role).await
    }

    pub async fn set_check_enabled(
        &self,
        admin: AdminId,
        role: AdminRole,
        enabled: bool,
    ) -> ServiceResult<()> {
        if role != AdminRole::Owner {
            return Err(ServiceError::Forbidden);
        }
        self.save(ENABLED_KEY, &enabled).await?;
        self.audit(Self::entry(
            Actor::Admin(admin),
            "system.settings_changed",
            serde_json::json!({ "check_enabled": enabled }),
            None,
        ))
        .await
    }

    pub async fn next_check_in(&self) -> ServiceResult<Option<std::time::Duration>> {
        let config = self.config();
        if !config.checks_allowed {
            return Ok(None);
        }
        let now = self.services.now();
        if !self.checks_enabled().await? {
            return Ok(Some(std::time::Duration::from_secs(3600)));
        }
        let state = self.check_state().await?;
        let mut rng = rand::thread_rng();
        let jitter = Jitter {
            soon: Duration::seconds(rng.gen_range(60..180)),
            spread: Duration::minutes(rng.gen_range(0..60)),
            stale: Duration::minutes(rng.gen_range(15..45)),
        };
        let due = check_due(&state, &build_key(&config.build), now, jitter);
        Ok(Some((due - now).to_std().unwrap_or_default()))
    }

    pub async fn scheduled_check(&self) -> ServiceResult<()> {
        let config = self.config();
        if !config.checks_allowed || !self.checks_enabled().await? {
            return Ok(());
        }
        self.run_check(&config).await.map(|_| ())
    }

    async fn deny(&self, admin: AdminId, reason: &str, meta: &RequestMeta) -> ServiceResult<()> {
        self.audit(Self::entry(
            Actor::Admin(admin),
            "system.update_denied",
            serde_json::json!({ "reason": reason }),
            Some(meta),
        ))
        .await
    }

    pub async fn request(
        &self,
        admin: AdminId,
        role: AdminRole,
        target: &str,
        password: &str,
        code: Option<&str>,
        meta: &RequestMeta,
    ) -> ServiceResult<Uuid> {
        if role != AdminRole::Owner {
            self.deny(admin, "not_owner", meta).await?;
            return Err(ServiceError::Forbidden);
        }
        if let Err(err) = self
            .services
            .auth()
            .confirm_admin(admin, password, code)
            .await
        {
            if matches!(err, ServiceError::Rejected { .. }) {
                self.deny(admin, err.code(), meta).await?;
            }
            return Err(err);
        }
        let config = self.config();
        let missing = || {
            ServiceError::rejected(
                HELPER_MISSING,
                "Обновление из админки на этом сервере выключено",
            )
        };
        let dir = config.dir.clone().ok_or_else(missing)?;
        let helper = read_helper(&dir).ok_or_else(missing)?;
        if !helper.panel_updates {
            return Err(missing());
        }
        if let Some((update, _)) = self.current_update(&config).await?
            && ACTIVE.contains(&update.state.as_str())
        {
            return Err(ServiceError::rejected(
                UPDATE_IN_PROGRESS,
                "Обновление уже идёт",
            ));
        }
        let stale = || {
            ServiceError::rejected(
                UPDATE_STALE,
                "Сведения о новой версии устарели, проверьте обновления ещё раз",
            )
        };
        let target = clean_commit(target).ok_or_else(stale)?;
        let check = self.check_state().await?;
        let latest = check
            .latest
            .filter(|b| is_newer(&config.build, b))
            .ok_or_else(stale)?;
        if latest.commit.as_deref() != Some(target.as_str())
            || helper.blocked.as_deref().and_then(clean_commit).as_deref() == Some(target.as_str())
        {
            return Err(stale());
        }
        let now = self.services.now();
        if self
            .marker()
            .await?
            .is_some_and(|m| now - m.requested_at < Duration::seconds(REQUEST_GAP_SECONDS))
        {
            return Err(ServiceError::rejected(
                UPDATE_RATE_LIMITED,
                "Обновление запрашивали только что, подождите полминуты",
            ));
        }
        let id = Uuid::new_v4();
        let line = format!(
            "{{\"id\":\"{id}\",\"action\":\"update\",\"target\":\"{target}\",\"requested_at\":\"{}\",\"admin_id\":{}}}\n",
            now.to_rfc3339_opts(SecondsFormat::Secs, true),
            admin.0
        );
        write_request(&dir, &id, line)
            .await
            .map_err(|e| ServiceError::Internal(format!("заявка не записана: {e}")))?;
        let marker = RequestMarker {
            id,
            target: target.clone(),
            from: config.build.commit.clone(),
            admin_id: admin.0,
            requested_at: now,
            state: MarkerState::Requested,
            from_label: config.build.label.clone(),
            to_label: latest.label.clone(),
        };
        self.save(REQUEST_KEY, &marker).await?;
        self.audit(Self::entry(
            Actor::Admin(admin),
            "system.update_requested",
            serde_json::json!({
                "id": id,
                "from": config.build.commit,
                "to": target,
                "user_agent": meta.user_agent,
            }),
            Some(meta),
        ))
        .await?;
        Ok(id)
    }

    pub async fn cancel(&self, admin: AdminId, role: AdminRole) -> ServiceResult<()> {
        if role != AdminRole::Owner {
            return Err(ServiceError::Forbidden);
        }
        let started =
            || ServiceError::rejected(UPDATE_STARTED, "Обновление уже началось, отменить нельзя");
        let config = self.config();
        let dir = config.dir.clone().ok_or_else(started)?;
        if !remove_request(&dir).await {
            return Err(started());
        }
        if let Some(mut marker) = self.marker().await? {
            marker.state = MarkerState::Canceled;
            self.save(REQUEST_KEY, &marker).await?;
            self.audit(Self::entry(
                Actor::Admin(admin),
                "system.update_canceled",
                serde_json::json!({ "id": marker.id, "reason": "owner" }),
                None,
            ))
            .await?;
        }
        Ok(())
    }
}

async fn remove_request(dir: &Path) -> bool {
    tokio::fs::remove_file(request_path(dir)).await.is_ok()
}

async fn write_request(dir: &Path, id: &Uuid, line: String) -> std::io::Result<()> {
    let folder = dir.join("request");
    let temp = folder.join(format!(".request.{id}.tmp"));
    let target = request_path(dir);
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temp).await?;
    use tokio::io::AsyncWriteExt;
    let written = async {
        file.write_all(line.as_bytes()).await?;
        file.sync_all().await
    }
    .await;
    drop(file);
    if let Err(err) = written {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(err);
    }
    if tokio::fs::try_exists(&target).await? {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "заявка уже лежит",
        ));
    }
    if let Err(err) = tokio::fs::rename(&temp, &target).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
