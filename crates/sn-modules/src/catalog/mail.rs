use crate::catalog::config_number;
use crate::error::ModuleError;
use crate::module::{Module, ModuleConfig};
use async_trait::async_trait;
use sn_domain::client::{IdentityKind, normalize_identity};
use sn_domain::module::ModuleId;
use sn_domain::settings::SettingField;

pub const TRANSPORT_OPTIONS: &[&str] = &["none", "smtp", "resend"];
pub const SECURITY_OPTIONS: &[&str] = &["starttls", "tls", "none"];
const NAME_MAX: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailSecurity {
    StartTls,
    Tls,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpSettings {
    pub host: String,
    pub port: u16,
    pub security: MailSecurity,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailTransport {
    Off,
    Smtp(SmtpSettings),
    Resend { api_key: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailSettings {
    pub transport: MailTransport,
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

fn smtp(config: &ModuleConfig) -> Result<SmtpSettings, String> {
    let host =
        optional_text(config, "smtp_host").ok_or_else(|| "не указан сервер SMTP".to_string())?;
    if host.contains(char::is_whitespace) || host.contains('/') || host.contains(':') {
        return Err("сервер SMTP записан неверно: нужно имя вроде smtp.example.com".into());
    }
    let security = match optional_text(config, "security").as_deref() {
        None | Some("starttls") => MailSecurity::StartTls,
        Some("tls") => MailSecurity::Tls,
        Some("none") => MailSecurity::Plain,
        Some(_) => return Err("шифрование: starttls, tls или none".into()),
    };
    let standard = match security {
        MailSecurity::Tls => 465,
        MailSecurity::StartTls => 587,
        MailSecurity::Plain => 25,
    };
    let port = match config.get("smtp_port") {
        None | Some(serde_json::Value::Null) => standard,
        Some(serde_json::Value::String(s)) if s.trim().is_empty() => standard,
        Some(_) => config_number(config, "smtp_port")
            .and_then(|n| u16::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| "порт — число от 1 до 65535".to_string())?,
    };
    let username = optional_text(config, "username");
    let password = config
        .text("password")
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    if password.is_some() && username.is_none() {
        return Err("пароль указан без логина".into());
    }
    Ok(SmtpSettings {
        host,
        port,
        security,
        username,
        password,
    })
}

impl MailSettings {
    pub fn parse(config: &ModuleConfig) -> Result<Self, String> {
        let transport = match optional_text(config, "transport").as_deref() {
            None | Some("none") => MailTransport::Off,
            Some("smtp") => MailTransport::Smtp(smtp(config)?),
            Some("resend") => MailTransport::Resend {
                api_key: optional_text(config, "resend_api_key")
                    .ok_or_else(|| "не указан ключ API Resend".to_string())?,
            },
            Some(_) => return Err("способ отправки: none, smtp или resend".into()),
        };
        let from_email = match optional_text(config, "from_email") {
            Some(raw) => normalize_identity(IdentityKind::Email, &raw)
                .map_err(|_| "адрес отправителя записан неверно".to_string())?,
            None if transport == MailTransport::Off => String::new(),
            None => return Err("не указан адрес отправителя".into()),
        };
        let from_name = optional_text(config, "from_name");
        if from_name
            .as_deref()
            .is_some_and(|n| n.chars().count() > NAME_MAX || n.contains(['<', '>', '"']))
        {
            return Err("имя отправителя — до 80 символов, без кавычек и угловых скобок".into());
        }
        Ok(MailSettings {
            transport,
            from_email,
            from_name,
        })
    }

    pub fn sends(&self) -> bool {
        self.transport != MailTransport::Off
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
            SettingField::select(
                "transport",
                "Подтверждение почты",
                "none — без подтверждения, вход по почте и паролю; smtp или resend — код письмом",
                TRANSPORT_OPTIONS,
            )
            .with_default("none"),
            SettingField::text(
                "smtp_host",
                "Сервер SMTP",
                "Например smtp.yandex.ru, smtp.mail.ru или smtp.gmail.com",
            )
            .optional(),
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
            SettingField::secret(
                "resend_api_key",
                "Ключ API Resend",
                "Начинается с re_, выдаётся в кабинете resend.com",
            )
            .optional(),
            SettingField::text(
                "from_email",
                "Адрес отправителя",
                "Его клиенты увидят в поле «От». Для Resend домен адреса должен быть подтверждён",
            )
            .optional(),
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
    fn по_умолчанию_письма_не_отправляются() {
        let off = MailSettings::parse(&config(json!({}))).unwrap();
        assert_eq!(off.transport, MailTransport::Off);
        assert!(!off.sends());
        let partial = MailSettings::parse(&config(json!({
            "transport": "none",
            "smtp_host": "недописанный адрес"
        })))
        .unwrap();
        assert!(!partial.sends());
    }

    #[test]
    fn smtp_проверяется_и_порт_берётся_по_шифрованию() {
        let ok = MailSettings::parse(&config(json!({
            "transport": "smtp",
            "smtp_host": "smtp.yandex.ru",
            "security": "tls",
            "username": "robot@example.com",
            "password": "app-password",
            "from_email": "Robot@Example.com",
            "from_name": "STEALTHNET"
        })))
        .unwrap();
        let MailTransport::Smtp(smtp) = &ok.transport else {
            panic!("ожидался SMTP");
        };
        assert_eq!(smtp.port, 465);
        assert_eq!(smtp.security, MailSecurity::Tls);
        assert_eq!(ok.from_email, "robot@example.com");
        let plain = MailSettings::parse(&config(json!({
            "transport": "smtp",
            "smtp_host": "mail.example.com",
            "smtp_port": "2525",
            "from_email": "robot@example.com"
        })))
        .unwrap();
        let MailTransport::Smtp(smtp) = &plain.transport else {
            panic!("ожидался SMTP");
        };
        assert_eq!(smtp.port, 2525);
        assert_eq!(smtp.security, MailSecurity::StartTls);
        for bad in [
            json!({ "transport": "smtp", "from_email": "robot@example.com" }),
            json!({ "transport": "smtp", "smtp_host": "smtp.example.com:587", "from_email": "robot@example.com" }),
            json!({ "transport": "smtp", "smtp_host": "smtp.example.com", "smtp_port": 70000, "from_email": "robot@example.com" }),
            json!({ "transport": "smtp", "smtp_host": "smtp.example.com" }),
            json!({ "transport": "smtp", "smtp_host": "smtp.example.com", "from_email": "не почта" }),
            json!({ "transport": "smtp", "smtp_host": "smtp.example.com", "from_email": "robot@example.com", "password": "x" }),
            json!({ "transport": "pigeon" }),
        ] {
            assert!(MailSettings::parse(&config(bad.clone())).is_err(), "{bad}");
        }
    }

    #[test]
    fn resend_требует_ключ_и_отправителя() {
        let ok = MailSettings::parse(&config(json!({
            "transport": "resend",
            "resend_api_key": "re_123",
            "from_email": "robot@example.com"
        })))
        .unwrap();
        assert_eq!(
            ok.transport,
            MailTransport::Resend {
                api_key: "re_123".into()
            }
        );
        assert!(
            MailSettings::parse(&config(
                json!({ "transport": "resend", "from_email": "robot@example.com" })
            ))
            .is_err()
        );
        assert!(
            MailSettings::parse(&config(
                json!({ "transport": "resend", "resend_api_key": "re_123" })
            ))
            .is_err()
        );
    }
}
