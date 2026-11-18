#[cfg(test)]
mod tests;

use crate::error::ModuleError;
use crate::module::{Module, ModuleConfig};
use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use sn_domain::client::IdentityKind;
use sn_domain::module::ModuleId;
use sn_domain::settings::SettingField;

pub const NOT_CONFIGURED: &str = "Не заданы ключи Google или Apple: кнопок входа в кабинете нет";
pub const TEMPLATE_LINKED: &str = "oauth_linked";
const PEM_BEGIN: &str = "-----BEGIN PRIVATE KEY-----";
const PEM_END: &str = "-----END PRIVATE KEY-----";
const GOOGLE_SUFFIX: &str = ".apps.googleusercontent.com";
const TEXT_MAX: usize = 200;
const KEY_MAX: usize = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Google,
    Apple,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Google, Provider::Apple];

    pub const fn as_str(self) -> &'static str {
        match self {
            Provider::Google => "google",
            Provider::Apple => "apple",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Provider::Google => "Google",
            Provider::Apple => "Apple",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Provider::ALL
            .into_iter()
            .find(|p| p.as_str() == value.trim())
    }

    pub const fn identity(self) -> IdentityKind {
        match self {
            Provider::Google => IdentityKind::Google,
            Provider::Apple => IdentityKind::Apple,
        }
    }

    pub const fn of(kind: IdentityKind) -> Option<Self> {
        match kind {
            IdentityKind::Google => Some(Provider::Google),
            IdentityKind::Apple => Some(Provider::Apple),
            IdentityKind::Telegram | IdentityKind::Email => None,
        }
    }

    pub fn callback_path(self) -> String {
        format!("/api/client/auth/oauth/{}/callback", self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleSettings {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleSettings {
    pub services_id: String,
    pub team_id: String,
    pub key_id: String,
    pub private_key: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OauthSettings {
    pub google: Option<GoogleSettings>,
    pub apple: Option<AppleSettings>,
}

fn field<'a>(config: &'a ModuleConfig, key: &str) -> Option<&'a str> {
    config.text(key)
}

fn plain(value: &str, what: &str) -> Result<String, String> {
    if value.chars().count() > TEXT_MAX || value.chars().any(char::is_whitespace) {
        return Err(format!("{what}: без пробелов, до {TEXT_MAX} символов"));
    }
    Ok(value.to_string())
}

fn apple_code(value: &str, what: &str) -> Result<String, String> {
    let code = value.trim().to_ascii_uppercase();
    if code.len() == 10 && code.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Ok(code)
    } else {
        Err(format!("{what}: 10 латинских букв и цифр"))
    }
}

pub fn normalize_key(raw: &str) -> Option<String> {
    let text = raw.replace("\\n", "\n").replace("\\r", "");
    let body = text
        .trim()
        .strip_prefix(PEM_BEGIN)
        .map(|rest| rest.trim_end().strip_suffix(PEM_END).unwrap_or(rest))
        .unwrap_or(text.trim());
    let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    if body.is_empty()
        || body.contains("-----")
        || !body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
    {
        return None;
    }
    let lines: Vec<String> = body
        .as_bytes()
        .chunks(64)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect();
    Some(format!("{PEM_BEGIN}\n{}\n{PEM_END}\n", lines.join("\n")))
}

pub fn check_key(pem: &str) -> Result<(), String> {
    let invalid = || {
        "ключ .p8 не подошёл: вставьте содержимое файла AuthKey_….p8 целиком, вместе со строками BEGIN и END".to_string()
    };
    let key = EncodingKey::from_ec_pem(pem.as_bytes()).map_err(|_| invalid())?;
    jsonwebtoken::encode(
        &Header::new(Algorithm::ES256),
        &serde_json::json!({ "check": true }),
        &key,
    )
    .map(|_| ())
    .map_err(|_| invalid())
}

impl OauthSettings {
    pub fn parse(config: &ModuleConfig) -> Result<Self, String> {
        Ok(OauthSettings {
            google: Self::google(config)?,
            apple: Self::apple(config)?,
        })
    }

    fn google(config: &ModuleConfig) -> Result<Option<GoogleSettings>, String> {
        let id = field(config, "google_client_id");
        let secret = field(config, "google_client_secret");
        let (id, secret) = match (id, secret) {
            (None, None) => return Ok(None),
            (Some(id), Some(secret)) => (id, secret),
            _ => return Err("Google: укажите и Client ID, и Client secret".into()),
        };
        let client_id = plain(id, "Google Client ID")?;
        if !client_id.ends_with(GOOGLE_SUFFIX) || client_id.len() == GOOGLE_SUFFIX.len() {
            return Err(format!(
                "Google Client ID выглядит как 1234-abc{GOOGLE_SUFFIX}: скопируйте его со страницы Credentials"
            ));
        }
        Ok(Some(GoogleSettings {
            client_id,
            client_secret: plain(secret, "Google Client secret")?,
        }))
    }

    fn apple(config: &ModuleConfig) -> Result<Option<AppleSettings>, String> {
        let values = [
            field(config, "apple_services_id"),
            field(config, "apple_team_id"),
            field(config, "apple_key_id"),
            field(config, "apple_private_key"),
        ];
        if values.iter().all(Option::is_none) {
            return Ok(None);
        }
        let [Some(services), Some(team), Some(key_id), Some(key)] = values else {
            return Err("Apple: нужны Services ID, Team ID, Key ID и ключ .p8".into());
        };
        let services_id = plain(services, "Apple Services ID")?;
        if !services_id.contains('.') {
            return Err(
                "Apple Services ID записывается как домен наоборот, например com.example.signin"
                    .into(),
            );
        }
        if key.len() > KEY_MAX {
            return Err("ключ .p8 слишком длинный: нужен файл AuthKey_….p8".into());
        }
        let private_key = normalize_key(key).ok_or_else(|| {
            "ключ .p8 не подошёл: вставьте содержимое файла AuthKey_….p8 целиком".to_string()
        })?;
        check_key(&private_key)?;
        Ok(Some(AppleSettings {
            services_id,
            team_id: apple_code(team, "Apple Team ID")?,
            key_id: apple_code(key_id, "Apple Key ID")?,
            private_key,
        }))
    }

    pub fn has(&self, provider: Provider) -> bool {
        match provider {
            Provider::Google => self.google.is_some(),
            Provider::Apple => self.apple.is_some(),
        }
    }

    pub fn any(&self) -> bool {
        self.google.is_some() || self.apple.is_some()
    }
}

pub struct OauthModule;

#[async_trait]
impl Module for OauthModule {
    fn id(&self) -> ModuleId {
        ModuleId::Oauth
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text(
                "google_client_id",
                "Google: Client ID",
                "Google Cloud Console → Google Auth Platform → Clients, клиент с типом Web application. Вид: 1234-abc.apps.googleusercontent.com",
            )
            .optional(),
            SettingField::secret(
                "google_client_secret",
                "Google: Client secret",
                "Там же, рядом с Client ID. Обычно начинается с GOCSPX-",
            )
            .optional(),
            SettingField::text(
                "apple_services_id",
                "Apple: Services ID",
                "developer.apple.com → Identifiers → Services IDs, например com.example.signin",
            )
            .optional(),
            SettingField::text(
                "apple_team_id",
                "Apple: Team ID",
                "10 знаков, виден справа вверху в аккаунте разработчика",
            )
            .optional(),
            SettingField::text(
                "apple_key_id",
                "Apple: Key ID",
                "Keys → ключ с включённым Sign in with Apple, 10 знаков",
            )
            .optional(),
            SettingField::secret(
                "apple_private_key",
                "Apple: ключ .p8",
                "Содержимое файла AuthKey_XXXXXXXXXX.p8 целиком, вместе со строками BEGIN и END",
            )
            .optional()
            .large(),
        ]
    }

    fn validate_config(&self, config: &ModuleConfig) -> Result<(), ModuleError> {
        OauthSettings::parse(config)
            .map(|_| ())
            .map_err(ModuleError::Invalid)
    }

    fn config_warning(&self, config: &ModuleConfig) -> Option<&'static str> {
        match OauthSettings::parse(config) {
            Ok(settings) if settings.any() => None,
            _ => Some(NOT_CONFIGURED),
        }
    }
}
