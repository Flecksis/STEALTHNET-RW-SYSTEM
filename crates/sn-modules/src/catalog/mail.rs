use crate::catalog::config_number;
use crate::error::ModuleError;
use crate::module::{Module, ModuleConfig};
use async_trait::async_trait;
use sn_domain::client::{IdentityKind, normalize_identity};
use sn_domain::mail_letters::Group;
use sn_domain::module::ModuleId;
use sn_domain::settings::SettingField;

pub const TRANSPORT_OPTIONS: &[&str] = &["none", "smtp", "resend"];
pub const SECURITY_OPTIONS: &[&str] = &["starttls", "tls", "none"];
pub const LOGIN_OPTIONS: &[&str] = &["password", "code"];
pub const NAME_MAX: usize = 80;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmailLogin {
    Password,
    Code,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientLetters {
    pub expiring: bool,
    pub payments: bool,
    pub support: bool,
}

impl ClientLetters {
    pub const fn allows(self, group: Group) -> bool {
        match group {
            Group::Payments => self.payments,
            Group::Renewal => self.expiring,
            Group::Support => self.support,
            Group::Login | Group::Service => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailSettings {
    pub transport: MailTransport,
    pub from_email: String,
    pub from_name: Option<String>,
    pub login: EmailLogin,
    pub letters: ClientLetters,
}

fn optional_text(config: &ModuleConfig, key: &str) -> Option<String> {
    config
        .text(key)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn security(config: &ModuleConfig) -> Result<MailSecurity, String> {
    match optional_text(config, "security").as_deref() {
        None | Some("starttls") => Ok(MailSecurity::StartTls),
        Some("tls") => Ok(MailSecurity::Tls),
        Some("none") => Ok(MailSecurity::Plain),
        Some(_) => Err("шифрование: starttls, tls или none".into()),
    }
}

fn port(config: &ModuleConfig, security: MailSecurity) -> Result<u16, String> {
    let standard = match security {
        MailSecurity::Tls => 465,
        MailSecurity::StartTls => 587,
        MailSecurity::Plain => 25,
    };
    match config.get("smtp_port") {
        None | Some(serde_json::Value::Null) => Ok(standard),
        Some(serde_json::Value::String(s)) if s.trim().is_empty() => Ok(standard),
        Some(_) => config_number(config, "smtp_port")
            .and_then(|n| u16::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| "порт — число от 1 до 65535".to_string()),
    }
}

fn smtp(config: &ModuleConfig) -> Result<SmtpSettings, String> {
    let host =
        optional_text(config, "smtp_host").ok_or_else(|| "не указан сервер SMTP".to_string())?;
    if host.contains(char::is_whitespace) || host.contains('/') || host.contains(':') {
        return Err("сервер SMTP записан неверно: нужно имя вроде smtp.example.com".into());
    }
    let security = security(config)?;
    let port = port(config, security)?;
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
        let login = match optional_text(config, "login").as_deref() {
            Some("password") => EmailLogin::Password,
            Some("code") => EmailLogin::Code,
            None if transport == MailTransport::Off => EmailLogin::Password,
            None => EmailLogin::Code,
            Some(_) => return Err("вход по почте: password или code".into()),
        };
        let flag = |key: &str| {
            config
                .get(key)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true)
        };
        let letters = ClientLetters {
            expiring: flag("letter_expiring"),
            payments: flag("letter_payments"),
            support: flag("letter_support"),
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
            login,
            letters,
        })
    }

    pub fn sends(&self) -> bool {
        self.transport != MailTransport::Off
    }

    pub fn code_login(&self) -> bool {
        self.sends() && self.login == EmailLogin::Code
    }
}

fn lowered(config: &ModuleConfig, key: &str) -> Option<String> {
    optional_text(config, key).map(|value| value.trim_end_matches('.').to_lowercase())
}

fn server_moved(before: &ModuleConfig, after: &ModuleConfig) -> Vec<&'static str> {
    let port_of = |config: &ModuleConfig| {
        port(config, security(config).unwrap_or(MailSecurity::StartTls)).ok()
    };
    [
        (
            "«Сервер SMTP»",
            lowered(before, "smtp_host") != lowered(after, "smtp_host"),
        ),
        ("«Порт»", port_of(before) != port_of(after)),
        (
            "«Шифрование»",
            security(before).ok() != security(after).ok(),
        ),
        (
            "«Логин»",
            lowered(before, "username") != lowered(after, "username"),
        ),
    ]
    .into_iter()
    .filter_map(|(label, moved)| moved.then_some(label))
    .collect()
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
                "Отправка писем",
                "«Не отправлять» — письма не уходят, и клиенты из бота не смогут привязать почту для входа на сайт: кнопки «Вход по почте» в боте не будет; «SMTP» — через ваш почтовый сервер; «Resend API» — через сервис Resend",
                TRANSPORT_OPTIONS,
            )
            .labels(&["Не отправлять", "SMTP", "Resend API"])
            .with_default("none"),
            SettingField::select(
                "login",
                "Вход на сайт по почте",
                "«По паролю» — клиент входит с паролем; «По коду из письма» — пароль не нужен, нужна отправка писем",
                LOGIN_OPTIONS,
            )
            .labels(&["По паролю", "По коду из письма"])
            .optional(),
            SettingField::flag(
                "letter_expiring",
                "Письма об автопродлении",
                "Не хватает денег на продление, продление не прошло, автопродление выключено",
            )
            .with_default("true"),
            SettingField::flag(
                "letter_payments",
                "Письма об оплате и подарках",
                "Подписка выдана или продлена, баланс пополнен, оплата на проверке или возвращена, подарки",
            )
            .with_default("true"),
            SettingField::flag(
                "letter_support",
                "Письмо с ответом поддержки",
                "Ответ на обращение со ссылкой на переписку в кабинете",
            )
            .with_default("true"),
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
            .labels(&["STARTTLS", "TLS", "Без шифрования"])
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

    fn validate_change(
        &self,
        before: &ModuleConfig,
        after: &ModuleConfig,
        new_secrets: &[&str],
    ) -> Result<(), ModuleError> {
        if after.text("password").is_none() || new_secrets.contains(&"password") {
            return Ok(());
        }
        let what = match server_moved(before, after).as_slice() {
            [] => return Ok(()),
            [one] => format!("поля {one}"),
            [rest @ .., last] => format!("полей {} и {last}", rest.join(", ")),
        };
        Err(ModuleError::SecretsAgain(format!(
            "При смене {what} введите пароль заново: панель не отправляет сохранённый пароль на другой сервер или под другим логином"
        )))
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
    fn вход_по_почте_настраивается_отдельно_от_отправки() {
        let off = MailSettings::parse(&config(json!({}))).unwrap();
        assert_eq!(off.login, EmailLogin::Password);
        assert!(!off.code_login());
        assert!(off.letters.expiring && off.letters.payments && off.letters.support);
        let smtp = json!({
            "transport": "smtp",
            "smtp_host": "smtp.example.com",
            "from_email": "robot@example.com"
        });
        let earlier = MailSettings::parse(&config(smtp.clone())).unwrap();
        assert_eq!(earlier.login, EmailLogin::Code);
        assert!(earlier.code_login());
        let mut chosen = smtp.clone();
        chosen["login"] = json!("password");
        chosen["letter_support"] = json!(false);
        let password = MailSettings::parse(&config(chosen)).unwrap();
        assert_eq!(password.login, EmailLogin::Password);
        assert!(!password.code_login());
        assert!(!password.letters.support && password.letters.payments);
        let silent = MailSettings::parse(&config(json!({ "login": "code" }))).unwrap();
        assert!(!silent.code_login());
        assert!(MailSettings::parse(&config(json!({ "login": "magic" }))).is_err());
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

    #[test]
    fn смена_сервера_smtp_требует_пароль_заново() {
        let saved = json!({
            "transport": "smtp",
            "smtp_host": "smtp.yandex.ru",
            "username": "robot@example.com",
            "password": "app-password",
            "from_email": "robot@example.com"
        });
        let with = |patch: serde_json::Value| {
            let mut value = saved.clone();
            for (key, field) in patch.as_object().unwrap() {
                value[key] = field.clone();
            }
            config(value)
        };
        let before = config(saved.clone());
        let refused = |after: &ModuleConfig| match MailModule
            .validate_change(&before, after, &[])
            .unwrap_err()
        {
            ModuleError::SecretsAgain(text) => text,
            other => panic!("ждали отказ «введите пароль заново», пришло {other}"),
        };
        let text = refused(&with(json!({ "smtp_host": "smtp.attacker.example" })));
        assert!(
            text.contains("поля «Сервер SMTP» введите пароль заново"),
            "{text}"
        );
        assert!(refused(&with(json!({ "smtp_port": 2525 }))).contains("«Порт»"));
        assert!(
            refused(&with(json!({ "security": "none" }))).contains("полей «Порт» и «Шифрование»")
        );
        assert!(refused(&with(json!({ "username": "other@example.com" }))).contains("«Логин»"));
        let moved = with(json!({ "smtp_host": "smtp.mail.ru" }));
        assert!(
            MailModule
                .validate_change(&before, &moved, &["password"])
                .is_ok()
        );
        let mut cleared = moved.to_value();
        cleared.as_object_mut().unwrap().remove("password");
        assert!(
            MailModule
                .validate_change(&before, &config(cleared), &[])
                .is_ok()
        );
        for same in [
            json!({ "smtp_host": " SMTP.Yandex.ru. ", "username": "Robot@Example.com" }),
            json!({ "smtp_port": 587 }),
            json!({ "security": "starttls", "smtp_port": "" }),
            json!({ "from_email": "news@example.com", "from_name": "Сервис" }),
            json!({ "transport": "none" }),
        ] {
            assert!(
                MailModule
                    .validate_change(&before, &with(same.clone()), &[])
                    .is_ok(),
                "{same}"
            );
        }
    }
}
