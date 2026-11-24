use crate::state::AppState;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::post;
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const REPORT_PATH: &str = "/api/public/csp-report";
pub const HSTS_MAX_AGE: u64 = 31_536_000;
const REPORT_LIMIT: usize = 16 * 1024;
const LOGGED_PER_WINDOW: u32 = 20;
const WINDOW: Duration = Duration::from_secs(60);
const FIELD_MAX: usize = 200;
const SAMPLE_MAX: usize = 40;
const LOOPBACK_FRAMES: &str = "http://localhost:* http://127.0.0.1:* http://*.localhost:*";
const EXTENSION_SCHEMES: [&str; 5] = [
    "chrome-extension:",
    "moz-extension:",
    "safari-extension:",
    "safari-web-extension:",
    "webkit-masked-url:",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CspMode {
    #[default]
    Admin,
    All,
    Report,
}

impl CspMode {
    pub fn parse(raw: &str) -> Option<CspMode> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "admin" => Some(CspMode::Admin),
            "all" => Some(CspMode::All),
            "report" => Some(CspMode::Report),
            _ => None,
        }
    }

    pub fn enforces(self, section: Section) -> bool {
        match self {
            CspMode::All => true,
            CspMode::Report => false,
            CspMode::Admin => section.is_admin(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Site,
    MiniApp,
    Admin,
    AdminFrame,
}

impl Section {
    fn is_admin(self) -> bool {
        matches!(self, Section::Admin | Section::AdminFrame)
    }

    fn ancestors(self) -> Option<&'static str> {
        match self {
            Section::MiniApp => None,
            Section::AdminFrame => Some("'self'"),
            Section::Site | Section::Admin => Some("'none'"),
        }
    }
}

pub fn policy(section: Section, loopback_frames: bool) -> String {
    let script = match section {
        Section::MiniApp => "'self' https://telegram.org",
        _ => "'self'",
    };
    let (style, img, frame) = if section.is_admin() {
        (
            "'self' 'unsafe-inline'",
            "'self' data: blob: https:",
            if loopback_frames {
                format!("'self' https: {LOOPBACK_FRAMES}")
            } else {
                "'self' https:".to_string()
            },
        )
    } else {
        ("'self'", "'self' data: blob:", "'none'".to_string())
    };
    let mut out = format!(
        "default-src 'self'; script-src {script}; style-src {style}; img-src {img}; \
         font-src 'self' data:; connect-src 'self'; frame-src {frame}; object-src 'none'; \
         base-uri 'none'; form-action 'self'"
    );
    if let Some(ancestors) = section.ancestors() {
        out.push_str("; frame-ancestors ");
        out.push_str(ancestors);
    }
    out
}

fn baseline(section: Section) -> String {
    let mut out = "object-src 'none'; base-uri 'none'".to_string();
    if let Some(ancestors) = section.ancestors() {
        out.push_str("; frame-ancestors ");
        out.push_str(ancestors);
    }
    out
}

pub fn apply(headers: &mut HeaderMap, section: Section, mode: CspMode, loopback_frames: bool) {
    let full = format!(
        "{}; report-uri {REPORT_PATH}",
        policy(section, loopback_frames)
    );
    let (enforced, report_only) = if mode.enforces(section) {
        (full, None)
    } else {
        (baseline(section), Some(full))
    };
    if let Ok(value) = HeaderValue::from_str(&enforced) {
        headers.insert(header::CONTENT_SECURITY_POLICY, value);
    }
    match report_only.and_then(|text| HeaderValue::from_str(&text).ok()) {
        Some(value) => {
            headers.insert(header::CONTENT_SECURITY_POLICY_REPORT_ONLY, value);
        }
        None => {
            headers.remove(header::CONTENT_SECURITY_POLICY_REPORT_ONLY);
        }
    }
}

fn first_value(headers: &HeaderMap, name: impl header::AsHeaderName) -> Option<&str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
}

pub fn over_https(headers: &HeaderMap) -> bool {
    if let Some(proto) = first_value(headers, "x-forwarded-proto") {
        return proto.eq_ignore_ascii_case("https");
    }
    first_value(headers, header::FORWARDED).is_some_and(|hop| {
        hop.split(';')
            .filter_map(|pair| pair.trim().split_once('='))
            .any(|(key, value)| {
                key.trim().eq_ignore_ascii_case("proto")
                    && value.trim().trim_matches('"').eq_ignore_ascii_case("https")
            })
    })
}

pub async fn transport_security(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let secure = over_https(request.headers());
    let mut response = next.run(request).await;
    if secure
        && !response
            .headers()
            .contains_key(header::STRICT_TRANSPORT_SECURITY)
        && let Ok(value) = HeaderValue::from_str(&format!("max-age={}", state.config.hsts_max_age))
    {
        response
            .headers_mut()
            .insert(header::STRICT_TRANSPORT_SECURITY, value);
    }
    response
}

pub fn routes() -> Router<AppState> {
    Router::new().route(
        REPORT_PATH,
        post(report).layer(DefaultBodyLimit::max(REPORT_LIMIT)),
    )
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "csp-report")]
    report: RawViolation,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
struct RawViolation {
    document_uri: Option<String>,
    effective_directive: Option<String>,
    violated_directive: Option<String>,
    blocked_uri: Option<String>,
    source_file: Option<String>,
    line_number: Option<u64>,
    script_sample: Option<String>,
    disposition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub enforced: Option<bool>,
    pub directive: String,
    pub blocked: String,
    pub page: String,
    pub source: String,
    pub line: Option<u64>,
    pub sample: String,
}

fn clip(text: &str, max: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect()
}

fn without_query(raw: &str) -> String {
    let raw = raw.trim();
    match raw.parse::<Uri>() {
        Ok(uri) => match (uri.scheme_str(), uri.authority()) {
            (Some(scheme), Some(authority)) => {
                clip(&format!("{scheme}://{authority}{}", uri.path()), FIELD_MAX)
            }
            _ if raw.contains(['?', '#']) => clip(uri.path(), FIELD_MAX),
            _ => clip(raw, FIELD_MAX),
        },
        Err(_) => clip(raw.split(['?', '#']).next().unwrap_or_default(), FIELD_MAX),
    }
}

fn page_path(raw: &str) -> String {
    match raw.trim().parse::<Uri>() {
        Ok(uri) => clip(uri.path(), FIELD_MAX),
        Err(_) => String::new(),
    }
}

fn from_extension(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    EXTENSION_SCHEMES
        .iter()
        .any(|scheme| value.starts_with(scheme))
}

impl Violation {
    pub fn parse(body: &[u8]) -> Option<Violation> {
        let raw = serde_json::from_slice::<Envelope>(body).ok()?.report;
        let blocked = raw.blocked_uri.unwrap_or_default();
        let source = raw.source_file.unwrap_or_default();
        if from_extension(&blocked) || from_extension(&source) {
            return None;
        }
        let directive = raw
            .effective_directive
            .or(raw.violated_directive)
            .unwrap_or_default();
        let directive = directive.split_whitespace().next().unwrap_or_default();
        if directive.is_empty() {
            return None;
        }
        Some(Violation {
            enforced: match raw.disposition.as_deref() {
                Some("enforce") => Some(true),
                Some("report") => Some(false),
                _ => None,
            },
            directive: clip(directive, 40),
            blocked: without_query(&blocked),
            page: page_path(&raw.document_uri.unwrap_or_default()),
            source: without_query(&source),
            line: raw.line_number,
            sample: clip(raw.script_sample.as_deref().unwrap_or_default(), SAMPLE_MAX),
        })
    }
}

struct Gate {
    since: Option<Instant>,
    logged: u32,
    skipped: u32,
}

impl Gate {
    const fn new() -> Gate {
        Gate {
            since: None,
            logged: 0,
            skipped: 0,
        }
    }

    fn admit(&mut self, now: Instant) -> Option<u32> {
        if self
            .since
            .is_none_or(|since| now.duration_since(since) >= WINDOW)
        {
            self.since = Some(now);
            self.logged = 0;
        }
        if self.logged >= LOGGED_PER_WINDOW {
            self.skipped = self.skipped.saturating_add(1);
            return None;
        }
        self.logged += 1;
        Some(std::mem::take(&mut self.skipped))
    }
}

static GATE: Mutex<Gate> = Mutex::new(Gate::new());

fn admit(now: Instant) -> Option<u32> {
    GATE.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .admit(now)
}

async fn report(body: Bytes) -> StatusCode {
    if let Some(violation) = Violation::parse(&body)
        && let Some(skipped) = admit(Instant::now())
    {
        tracing::warn!(
            blocked_by_browser = violation.enforced,
            directive = %violation.directive,
            blocked = %violation.blocked,
            page = %violation.page,
            source = %violation.source,
            line = violation.line,
            sample = %violation.sample,
            skipped_before = skipped,
            "CSP: браузер сообщил о нарушении политики страницы"
        );
    }
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn режим_csp_читается_из_переменной() {
        assert_eq!(CspMode::parse("admin"), Some(CspMode::Admin));
        assert_eq!(CspMode::parse(" ALL "), Some(CspMode::All));
        assert_eq!(CspMode::parse("report"), Some(CspMode::Report));
        assert_eq!(CspMode::parse("enforce"), None);
        assert_eq!(CspMode::default(), CspMode::Admin);
        assert!(CspMode::Admin.enforces(Section::Admin));
        assert!(CspMode::Admin.enforces(Section::AdminFrame));
        assert!(!CspMode::Admin.enforces(Section::Site));
        assert!(!CspMode::Admin.enforces(Section::MiniApp));
        assert!(CspMode::All.enforces(Section::MiniApp));
        assert!(!CspMode::Report.enforces(Section::Admin));
    }

    #[test]
    fn скрипты_только_свои_а_у_мини_приложения_ещё_telegram() {
        for section in [Section::Site, Section::Admin, Section::AdminFrame] {
            let text = policy(section, false);
            assert!(text.contains("script-src 'self';"), "{section:?}: {text}");
            assert!(!text.contains("unsafe-eval"), "{section:?}: {text}");
            assert!(text.contains("object-src 'none'"), "{section:?}: {text}");
            assert!(text.contains("base-uri 'none'"), "{section:?}: {text}");
        }
        let mini = policy(Section::MiniApp, false);
        assert!(mini.contains("script-src 'self' https://telegram.org;"));
        assert!(!mini.contains("frame-ancestors"));
        assert!(!mini.contains("unsafe-inline"));
    }

    #[test]
    fn стили_и_картинки_со_стороны_только_в_админке() {
        let site = policy(Section::Site, false);
        assert!(site.contains("style-src 'self';"));
        assert!(site.contains("img-src 'self' data: blob:;"));
        assert!(site.contains("frame-src 'none'"));
        assert!(site.ends_with("frame-ancestors 'none'"));
        let admin = policy(Section::Admin, false);
        assert!(admin.contains("style-src 'self' 'unsafe-inline';"));
        assert!(admin.contains("img-src 'self' data: blob: https:;"));
        assert!(admin.contains("frame-src 'self' https:;"));
        assert!(!admin.contains("localhost"));
        assert!(admin.ends_with("frame-ancestors 'none'"));
        assert!(policy(Section::AdminFrame, false).ends_with("frame-ancestors 'self'"));
        let loopback = policy(Section::Admin, true);
        assert!(loopback.contains(
            "frame-src 'self' https: http://localhost:* http://127.0.0.1:* http://*.localhost:*;"
        ));
        assert!(!policy(Section::Site, true).contains("localhost"));
    }

    #[test]
    fn вне_админки_по_умолчанию_только_отчёты_и_базовые_запреты() {
        let mut headers = HeaderMap::new();
        apply(&mut headers, Section::Site, CspMode::Admin, false);
        assert_eq!(
            headers[header::CONTENT_SECURITY_POLICY],
            "object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
        );
        let report = headers[header::CONTENT_SECURITY_POLICY_REPORT_ONLY]
            .to_str()
            .unwrap();
        assert!(report.starts_with("default-src 'self'; script-src 'self';"));
        assert!(report.ends_with("; report-uri /api/public/csp-report"));
        apply(&mut headers, Section::Site, CspMode::All, false);
        assert!(!headers.contains_key(header::CONTENT_SECURITY_POLICY_REPORT_ONLY));
        assert!(
            headers[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .starts_with("default-src 'self';")
        );
        let mut mini = HeaderMap::new();
        apply(&mut mini, Section::MiniApp, CspMode::Admin, false);
        assert_eq!(
            mini[header::CONTENT_SECURITY_POLICY],
            "object-src 'none'; base-uri 'none'"
        );
    }

    #[test]
    fn https_узнаётся_по_заголовкам_прокси() {
        let mut headers = HeaderMap::new();
        assert!(!over_https(&headers));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert!(over_https(&headers));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("HTTPS, http"));
        assert!(over_https(&headers));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("http"));
        assert!(!over_https(&headers));
        let mut forwarded = HeaderMap::new();
        forwarded.insert(
            header::FORWARDED,
            HeaderValue::from_static("for=203.0.113.7;proto=https;host=vpn.example"),
        );
        assert!(over_https(&forwarded));
        forwarded.insert(
            header::FORWARDED,
            HeaderValue::from_static("for=203.0.113.7;proto=http, for=10.0.0.2;proto=https"),
        );
        assert!(!over_https(&forwarded));
    }

    #[test]
    fn отчёт_браузера_разбирается_без_адресов_с_параметрами() {
        let body = serde_json::json!({
            "csp-report": {
                "document-uri": "https://vpn.example/cabinet/pay?token=SECRET#frag",
                "effective-directive": "script-src-elem",
                "violated-directive": "script-src-elem",
                "blocked-uri": "https://static.example-cdn.com/beacon.min.js?key=SECRET",
                "source-file": "https://vpn.example/assets/index-1a2b.js?v=SECRET",
                "line-number": 12,
                "script-sample": "",
                "disposition": "report"
            }
        });
        let violation = Violation::parse(body.to_string().as_bytes()).unwrap();
        assert_eq!(
            violation,
            Violation {
                enforced: Some(false),
                directive: "script-src-elem".into(),
                blocked: "https://static.example-cdn.com/beacon.min.js".into(),
                page: "/cabinet/pay".into(),
                source: "https://vpn.example/assets/index-1a2b.js".into(),
                line: Some(12),
                sample: String::new(),
            }
        );
        let inline = serde_json::json!({
            "csp-report": {
                "document-uri": "https://vpn.example/",
                "violated-directive": "style-src-attr 'self'",
                "blocked-uri": "inline",
                "script-sample": "color:\nred; very long sample that goes on and on",
            }
        });
        let violation = Violation::parse(inline.to_string().as_bytes()).unwrap();
        assert_eq!(violation.enforced, None);
        assert_eq!(violation.directive, "style-src-attr");
        assert_eq!(violation.blocked, "inline");
        assert_eq!(violation.sample.chars().count(), SAMPLE_MAX);
        assert!(!violation.sample.contains('\n'));
        let extension = serde_json::json!({
            "csp-report": {
                "document-uri": "https://vpn.example/admin",
                "effective-directive": "script-src-elem",
                "blocked-uri": "chrome-extension://abcdef/inject.js",
            }
        });
        assert_eq!(Violation::parse(extension.to_string().as_bytes()), None);
        let blocked = serde_json::json!({
            "csp-report": {
                "document-uri": "https://vpn.example/admin",
                "effective-directive": "frame-src",
                "blocked-uri": "http://intranet.example/",
                "disposition": "enforce"
            }
        });
        assert_eq!(
            Violation::parse(blocked.to_string().as_bytes())
                .unwrap()
                .enforced,
            Some(true)
        );
        assert_eq!(Violation::parse(b"not json"), None);
        assert_eq!(Violation::parse(b"{\"csp-report\":{}}"), None);
    }

    #[test]
    fn журнал_отчётов_ограничен_в_минуту() {
        let mut gate = Gate::new();
        let start = Instant::now();
        let admitted = (0..LOGGED_PER_WINDOW + 5)
            .filter(|_| gate.admit(start).is_some())
            .count();
        assert_eq!(admitted, LOGGED_PER_WINDOW as usize);
        assert_eq!(gate.admit(start + WINDOW / 2), None);
        assert_eq!(gate.admit(start + WINDOW), Some(6));
        assert_eq!(gate.admit(start + WINDOW), Some(0));
    }
}
