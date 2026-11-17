use crate::DomainError;
use crate::ids::{ClientId, TelegramId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientStatus {
    #[default]
    Active,
    Blocked,
}

impl ClientStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            ClientStatus::Active => "active",
            ClientStatus::Blocked => "blocked",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "active" => Ok(ClientStatus::Active),
            "blocked" => Ok(ClientStatus::Blocked),
            other => Err(DomainError::unknown("client_status", other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Locale {
    #[default]
    Ru,
    En,
}

impl Locale {
    pub const fn as_str(self) -> &'static str {
        match self {
            Locale::Ru => "ru",
            Locale::En => "en",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "ru" => Ok(Locale::Ru),
            "en" => Ok(Locale::En),
            other => Err(DomainError::unknown("locale", other)),
        }
    }

    pub fn from_telegram(language_code: Option<&str>) -> Self {
        match language_code.map(|c| c.split(['-', '_']).next().unwrap_or(c)) {
            Some("ru" | "uk" | "be" | "kk") | None => Locale::Ru,
            Some(_) => Locale::En,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationSource {
    Bot,
    Miniapp,
    Web,
    Admin,
    Import,
    Migration,
}

impl RegistrationSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            RegistrationSource::Bot => "bot",
            RegistrationSource::Miniapp => "miniapp",
            RegistrationSource::Web => "web",
            RegistrationSource::Admin => "admin",
            RegistrationSource::Import => "import",
            RegistrationSource::Migration => "migration",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "bot" => Ok(RegistrationSource::Bot),
            "miniapp" => Ok(RegistrationSource::Miniapp),
            "web" => Ok(RegistrationSource::Web),
            "admin" => Ok(RegistrationSource::Admin),
            "import" => Ok(RegistrationSource::Import),
            "migration" => Ok(RegistrationSource::Migration),
            other => Err(DomainError::unknown("registration_source", other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Client {
    pub id: ClientId,
    pub public_id: Uuid,
    pub status: ClientStatus,
    pub block_reason: Option<String>,
    pub locale: Locale,
    pub note: Option<String>,
    pub registration_source: RegistrationSource,
    pub bot_unreachable: bool,
    pub created_at: DateTime<Utc>,
    pub blocked_at: Option<DateTime<Utc>>,
}

impl Client {
    pub const fn is_blocked(&self) -> bool {
        matches!(self.status, ClientStatus::Blocked)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityKind {
    Telegram,
    Email,
    Google,
    Apple,
}

impl IdentityKind {
    pub const ALL: [IdentityKind; 4] = [
        IdentityKind::Telegram,
        IdentityKind::Email,
        IdentityKind::Google,
        IdentityKind::Apple,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            IdentityKind::Telegram => "telegram",
            IdentityKind::Email => "email",
            IdentityKind::Google => "google",
            IdentityKind::Apple => "apple",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        IdentityKind::ALL
            .into_iter()
            .find(|k| k.as_str() == value)
            .ok_or_else(|| DomainError::unknown("identity_kind", value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub kind: IdentityKind,
    pub value: String,
    pub verified: bool,
    pub meta: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewIdentity {
    pub kind: IdentityKind,
    pub value: String,
    pub verified: bool,
    pub meta: serde_json::Value,
}

impl NewIdentity {
    pub fn new(kind: IdentityKind, raw: &str, verified: bool) -> Result<Self, DomainError> {
        Ok(NewIdentity {
            kind,
            value: normalize_identity(kind, raw)?,
            verified,
            meta: serde_json::Value::Object(Default::default()),
        })
    }

    pub fn telegram(profile: &TelegramProfile) -> Self {
        NewIdentity {
            kind: IdentityKind::Telegram,
            value: profile.id.to_string(),
            verified: true,
            meta: serde_json::json!({
                "username": profile.username,
                "first_name": profile.first_name,
                "last_name": profile.last_name,
                "language_code": profile.language_code,
                "is_premium": profile.is_premium,
            }),
        }
    }
}

pub fn normalize_identity(kind: IdentityKind, raw: &str) -> Result<String, DomainError> {
    let value = raw.trim();
    match kind {
        IdentityKind::Telegram => {
            let id: i64 = value
                .parse()
                .map_err(|_| DomainError::invalid("telegram_id", "ожидается число"))?;
            if id <= 0 {
                return Err(DomainError::invalid(
                    "telegram_id",
                    "ожидается положительное число",
                ));
            }
            Ok(id.to_string())
        }
        IdentityKind::Email => {
            let lower = value.to_lowercase();
            let valid = lower.len() <= 254
                && lower.split_once('@').is_some_and(|(local, domain)| {
                    !local.is_empty()
                        && domain.contains('.')
                        && !domain.starts_with('.')
                        && !domain.ends_with('.')
                        && !lower.contains(char::is_whitespace)
                        && lower.matches('@').count() == 1
                });
            if valid {
                Ok(lower)
            } else {
                Err(DomainError::invalid("email", "адрес почты указан неверно"))
            }
        }
        IdentityKind::Google | IdentityKind::Apple => {
            if value.is_empty() || value.len() > 255 {
                Err(DomainError::invalid(
                    "subject",
                    "пустой или слишком длинный идентификатор",
                ))
            } else {
                Ok(value.to_string())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramProfile {
    pub id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub language_code: Option<String>,
    pub is_premium: bool,
}

impl TelegramProfile {
    pub const fn telegram_id(&self) -> TelegramId {
        TelegramId(self.id)
    }

    pub fn display_name(&self) -> String {
        let full = [self.first_name.as_deref(), self.last_name.as_deref()]
            .into_iter()
            .flatten()
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if !full.is_empty() {
            full
        } else if let Some(username) = &self.username {
            format!("@{username}")
        } else {
            self.id.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn почта_приводится_к_нижнему_регистру() {
        assert_eq!(
            normalize_identity(IdentityKind::Email, "  Alex@Example.COM ").unwrap(),
            "alex@example.com"
        );
        for bad in [
            "alex",
            "@example.com",
            "alex@example",
            "a b@example.com",
            "a@@b.com",
        ] {
            assert!(
                normalize_identity(IdentityKind::Email, bad).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn telegram_id_только_положительное_число() {
        assert_eq!(
            normalize_identity(IdentityKind::Telegram, "5093118422").unwrap(),
            "5093118422"
        );
        assert!(normalize_identity(IdentityKind::Telegram, "-100").is_err());
        assert!(normalize_identity(IdentityKind::Telegram, "abc").is_err());
    }

    #[test]
    fn язык_по_telegram() {
        assert_eq!(Locale::from_telegram(Some("ru")), Locale::Ru);
        assert_eq!(Locale::from_telegram(Some("en-US")), Locale::En);
        assert_eq!(Locale::from_telegram(None), Locale::Ru);
    }
}
