use crate::catalog::config_number;
use crate::error::ModuleError;
use crate::module::{Module, ModuleConfig};
use async_trait::async_trait;
use sn_domain::client::{IdentityKind, normalize_identity};
use sn_domain::module::ModuleId;
use sn_domain::settings::SettingField;

pub const SECURITY_OPTIONS: &[&str] = &["starttls", "tls", "none"];
const NAME_MAX: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailSecurity {
    StartTls,
    Tls,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailSettings {
    pub host: String,
    pub port: u16,
    pub security: MailSecurity,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_email: String,
    pub from_name: Option<String>,
}

fn optional_text(config: &ModuleConfig, key: &str) -> Option<String> {
    config
        .text(key)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

impl MailSettings {
    pub fn parse(config: &ModuleConfig) -> Result<Self, String> {
        let host = optional_text(config, "smtp_host")
            .ok_or_else(|| "не указан сервер SMTP".to_string())?;
        if host.contains(char::is_whitespace) || host.contains('/') || host.contains(':') {
            return Err("сервер SMTP записан неверно: нужно имя вроде smtp.example.com".into());
        }
        let security = match optional_text(config, "security").as_deref() {
            None | Some("starttls") => MailSecurity::StartTls,
            Some("tls") => MailSecurity::Tls,
            Some("none") => MailSecurity::Plain,
            Some(_) => return Err("шифрование: starttls, tls или none".into()),
        };
        let port = match config.get("smtp_port") {
            None | Some(serde_json::Value::Null) => match security {
                MailSecurity::Tls => 465,
                MailSecurity::StartTls => 587,
                MailSecurity::Plain => 25,
            },
            Some(serde_json::Value::String(s)) if s.trim().is_empty() => match security {
                MailSecurity::Tls => 465,
                MailSecurity::StartTls => 587,
                MailSecurity::Plain => 25,
            },
            Some(_) => config_number(config, "smtp_port")
                .and_then(|n| u16::try_from(n).ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| "порт — число от 1 до 65535".to_string())?,
        };
        let from_email = optional_text(config, "from_email")
            .ok_or_else(|| "не указан адрес отправителя".to_string())?;
        let from_email = normalize_identity(IdentityKind::Email, &from_email)
            .map_err(|_| "адрес отправителя записан неверно".to_string())?;
        let from_name = optional_text(config, "from_name");
        if from_name
            .as_deref()
            .is_some_and(|n| n.chars().count() > NAME_MAX || n.contains(['<', '>', '"']))
        {
            return Err("имя отправителя — до 80 символов, без кавычек и угловых скобок".into());
        }
        let username = optional_text(config, "username");
        let password = config
            .text("password")
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        if password.is_some() && username.is_none() {
            return Err("пароль указан без логина".into());
        }
        Ok(MailSettings {
            host,
            port,
            security,
            username,
            password,
            from_email,
            from_name,
        })
    }
}

pub struct MailModule;

#[async_trait]
impl Module for MailModule {
    fn id(&self) -> ModuleId {
        ModuleId::Mail
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text(
                "smtp_host",
                "Сервер SMTP",
                "Например smtp.yandex.ru, smtp.mail.ru или smtp.gmail.com",
            ),
            SettingField::select(
                "security",
                "Шифрование",
                "STARTTLS для порта 587, TLS для порта 465",
                SECURITY_OPTIONS,
            )
            .with_default("starttls"),
            SettingField::number("smtp_port", "Порт", "Пусто — 587 для STARTTLS, 465 для TLS"),
            SettingField::text(
                "username",
                "Логин",
                "Обычно это адрес почты, с которой идут письма",
            )
            .optional(),
            SettingField::secret(
                "password",
                "Пароль",
                "Для Яндекса, Mail.ru и Gmail — пароль приложения, а не от почты",
            )
            .optional(),
            SettingField::text(
                "from_email",
                "Адрес отправителя",
                "Его клиенты увидят в поле «От»",
            ),
            SettingField::text("from_name", "Имя отправителя", "Например название сервиса")
                .optional(),
        ]
    }

    fn validate_config(&self, config: &ModuleConfig) -> Result<(), ModuleError> {
        MailSettings::parse(config)
            .map(|_| ())
            .map_err(ModuleError::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(value: serde_json::Value) -> ModuleConfig {
        ModuleConfig::from_value(&value)
    }

    #[test]
    fn настройки_почты_проверяются_и_порт_берётся_по_шифрованию() {
        let ok = MailSettings::parse(&config(json!({
            "smtp_host": "smtp.yandex.ru",
            "security": "tls",
            "username": "robot@example.com",
            "password": "app-password",
            "from_email": "Robot@Example.com",
            "from_name": "STEALTHNET"
        })))
        .unwrap();
        assert_eq!(ok.port, 465);
        assert_eq!(ok.security, MailSecurity::Tls);
        assert_eq!(ok.from_email, "robot@example.com");
        let plain = MailSettings::parse(&config(json!({
            "smtp_host": "mail.example.com",
            "smtp_port": "2525",
            "from_email": "robot@example.com"
        })))
        .unwrap();
        assert_eq!(plain.port, 2525);
        assert_eq!(plain.security, MailSecurity::StartTls);
        assert_eq!(plain.username, None);
        for bad in [
            json!({ "from_email": "robot@example.com" }),
            json!({ "smtp_host": "smtp.example.com:587", "from_email": "robot@example.com" }),
            json!({ "smtp_host": "smtp.example.com", "smtp_port": 70000, "from_email": "robot@example.com" }),
            json!({ "smtp_host": "smtp.example.com", "from_email": "не почта" }),
            json!({ "smtp_host": "smtp.example.com", "from_email": "robot@example.com", "password": "x" }),
            json!({ "smtp_host": "smtp.example.com", "from_email": "robot@example.com", "security": "ssl3" }),
        ] {
            assert!(MailSettings::parse(&config(bad.clone())).is_err(), "{bad}");
        }
    }
}
