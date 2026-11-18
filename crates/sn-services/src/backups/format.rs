use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const FORMAT: &str = "stealthnet-backup";
pub const FORMAT_VERSION: u32 = 1;
pub const PLAIN_EXT: &str = ".tar.gz";
pub const SEALED_EXT: &str = ".tar.gz.enc";
pub const PREFIX: &str = "stealthnet-backup-";
pub const PARTIAL_PREFIX: &str = ".partial-";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    Auto,
    Manual,
    BeforeRestore,
    Restore,
}

impl RunKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            RunKind::Auto => "auto",
            RunKind::Manual => "manual",
            RunKind::BeforeRestore => "before_restore",
            RunKind::Restore => "restore",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [
            RunKind::Auto,
            RunKind::Manual,
            RunKind::BeforeRestore,
            RunKind::Restore,
        ]
        .into_iter()
        .find(|k| k.as_str() == value)
    }

    pub const fn file_tag(self) -> &'static str {
        match self {
            RunKind::Auto => "auto",
            RunKind::Manual => "manual",
            RunKind::BeforeRestore | RunKind::Restore => "before-restore",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "auto" => Some(RunKind::Auto),
            "manual" => Some(RunKind::Manual),
            "before-restore" => Some(RunKind::BeforeRestore),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub version: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
}

impl AppInfo {
    pub fn shown(&self) -> String {
        self.label.clone().unwrap_or_else(|| self.version.clone())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    #[serde(default)]
    pub clients: u64,
    #[serde(default)]
    pub subscriptions: u64,
    #[serde(default)]
    pub active_subscriptions: Option<u64>,
    #[serde(default)]
    pub payments: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableEntry {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceEntry {
    pub name: String,
    pub value: i64,
    pub called: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyInfo {
    pub present: bool,
    pub included: bool,
    #[serde(default)]
    pub check: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemnaInfo {
    pub source: String,
    pub taken_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub users: u64,
    #[serde(default)]
    pub linked: u64,
    #[serde(default)]
    pub missing: u64,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub format_version: u32,
    pub created_at: DateTime<Utc>,
    pub kind: RunKind,
    pub app: AppInfo,
    #[serde(default)]
    pub postgres: Option<String>,
    #[serde(default)]
    pub brand: Option<String>,
    #[serde(default)]
    pub public_url: Option<String>,
    pub migrations: Vec<i64>,
    pub counts: Counts,
    pub tables: Vec<TableEntry>,
    #[serde(default)]
    pub sequences: Vec<SequenceEntry>,
    pub secret_key: KeyInfo,
    #[serde(default)]
    pub remnawave: Option<RemnaInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Public {
    pub format: String,
    pub format_version: u32,
    pub created_at: DateTime<Utc>,
    pub kind: RunKind,
    pub app: String,
    pub key_included: bool,
}

impl Manifest {
    pub fn public(&self) -> Public {
        Public {
            format: self.format.clone(),
            format_version: self.format_version,
            created_at: self.created_at,
            kind: self.kind,
            app: self.app.shown(),
            key_included: self.secret_key.included,
        }
    }

    pub fn migration_max(&self) -> Option<i64> {
        self.migrations.iter().copied().max()
    }
}

pub fn file_name(at: DateTime<Utc>, kind: RunKind, sealed: bool) -> String {
    numbered_name(at, kind, sealed, 0)
}

pub fn numbered_name(at: DateTime<Utc>, kind: RunKind, sealed: bool, n: u32) -> String {
    let tail = if n == 0 {
        String::new()
    } else {
        format!("-{n}")
    };
    format!(
        "{PREFIX}{}-{}{tail}{}",
        at.format("%Y%m%d-%H%M%S"),
        kind.file_tag(),
        if sealed { SEALED_EXT } else { PLAIN_EXT }
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameInfo {
    pub at: DateTime<Utc>,
    pub kind: RunKind,
    pub sealed: bool,
}

pub fn parse_name(name: &str) -> Option<NameInfo> {
    let rest = name.strip_prefix(PREFIX)?;
    let (rest, sealed) = if let Some(rest) = rest.strip_suffix(SEALED_EXT) {
        (rest, true)
    } else {
        (rest.strip_suffix(PLAIN_EXT)?, false)
    };
    if rest.len() < 16 || !rest.is_char_boundary(15) {
        return None;
    }
    let (stamp, tag) = rest.split_at(15);
    let tag = tag.strip_prefix('-')?;
    let at = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%d-%H%M%S")
        .ok()?
        .and_utc();
    let kind = RunKind::from_tag(tag).or_else(|| {
        let (tag, n) = tag.rsplit_once('-')?;
        (!n.is_empty() && n.len() <= 4 && n.bytes().all(|b| b.is_ascii_digit()))
            .then(|| RunKind::from_tag(tag))
            .flatten()
    })?;
    Some(NameInfo { at, kind, sealed })
}

pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && parse_name(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn имя_файла_копии_разбирается_обратно() {
        let at = Utc.with_ymd_and_hms(2026, 11, 18, 4, 0, 12).unwrap();
        let name = file_name(at, RunKind::Auto, true);
        assert_eq!(name, "stealthnet-backup-20261118-040012-auto.tar.gz.enc");
        let info = parse_name(&name).unwrap();
        assert_eq!(info.at, at);
        assert_eq!(info.kind, RunKind::Auto);
        assert!(info.sealed);
        let plain = file_name(at, RunKind::BeforeRestore, false);
        assert_eq!(
            plain,
            "stealthnet-backup-20261118-040012-before-restore.tar.gz"
        );
        assert_eq!(parse_name(&plain).unwrap().kind, RunKind::BeforeRestore);
        assert!(safe_name(&plain));
        assert!(!safe_name("../etc/passwd"));
        assert!(!safe_name(
            "stealthnet-backup-20261118-040012-auto.tar.gz/../x"
        ));
        assert!(!safe_name("stealthnet-backup-2026111-040012-auto.tar.gz"));
        assert!(!safe_name("stealthnet-backup-20261118-040012-other.tar.gz"));
        assert!(!safe_name(
            ".partial-stealthnet-backup-20261118-040012-auto.tar.gz"
        ));
        let second = numbered_name(at, RunKind::Manual, false, 2);
        assert_eq!(second, "stealthnet-backup-20261118-040012-manual-2.tar.gz");
        assert_eq!(parse_name(&second).unwrap().kind, RunKind::Manual);
        assert!(parse_name("stealthnet-backup-20261118-040012-manual-x.tar.gz").is_none());
    }
}
