use crate::Services;
use crate::error::ServiceResult;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sn_domain::client::{IdentityKind, normalize_identity};
use sn_domain::module::ModuleId;
use sn_modules::ModuleConfig;
use sn_modules::catalog::mail::{MailSecurity, MailSettings, MailTransport};
use std::collections::BTreeMap;

pub const SMTP_FIELDS: &[&str] = &["smtp_host", "smtp_port", "security", "username", "password"];
pub const RESEND_FIELDS: &[&str] = &["resend_api_key"];
const NAME_MAX: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum MailPlan {
    Transfer { incoming: String },
    Same { incoming: String },
    Differs { current: String, incoming: String },
    Skipped { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldMail {
    pub config: Map<String, Value>,
    pub summary: String,
    pub resend: bool,
    fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Old {
    Absent,
    Broken(String),
    Ready(OldMail),
}

fn text<'a>(old: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    old.get(key).map(|v| v.trim()).filter(|v| !v.is_empty())
}

fn truthy(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

fn clean_host(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    let mut host = raw;
    for scheme in ["smtps://", "smtp://"] {
        if lower.starts_with(scheme) {
            host = &raw[scheme.len()..];
        }
    }
    let host = host.trim().trim_end_matches('/');
    match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            name.to_string()
        }
        _ => host.to_string(),
    }
}

fn sender_name(raw: &str) -> Option<String> {
    let name: String = raw
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | '"'))
        .collect::<String>()
        .trim()
        .chars()
        .take(NAME_MAX)
        .collect();
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

fn summary(settings: &MailSettings) -> String {
    let from = &settings.from_email;
    match &settings.transport {
        MailTransport::Off => "отправка писем выключена".to_string(),
        MailTransport::Smtp(smtp) => {
            let security = match smtp.security {
                MailSecurity::Tls => "TLS",
                MailSecurity::StartTls => "STARTTLS",
                MailSecurity::Plain => "без шифрования",
            };
            format!(
                "SMTP {}:{} ({security}), отправитель {from}",
                smtp.host, smtp.port
            )
        }
        MailTransport::Resend { .. } => format!("Resend, отправитель {from}"),
    }
}

fn fingerprint(settings: &MailSettings) -> String {
    let name = settings.from_name.as_deref().unwrap_or_default();
    match &settings.transport {
        MailTransport::Off => "off".to_string(),
        MailTransport::Smtp(smtp) => format!(
            "smtp|{}|{}|{:?}|{}|{}|{name}",
            smtp.host.to_ascii_lowercase(),
            smtp.port,
            smtp.security,
            smtp.username.as_deref().unwrap_or_default(),
            settings.from_email
        ),
        MailTransport::Resend { .. } => format!("resend|{}|{name}", settings.from_email),
    }
}

pub fn old_mail(old: &BTreeMap<String, String>) -> Old {
    let any = [
        "smtp_host",
        "smtp_user",
        "smtp_password",
        "smtp_from_email",
        "resend_api_key",
        "resend_from_email",
    ]
    .iter()
    .any(|key| text(old, key).is_some());
    if !any {
        return Old::Absent;
    }
    let resend = text(old, "mail_provider").is_some_and(|p| p.eq_ignore_ascii_case("resend"));
    let mut config = Map::new();
    let from = if resend {
        let Some(key) = text(old, "resend_api_key") else {
            return Old::Broken(
                "в старой панели выбрана отправка через Resend, но не задан ключ API".into(),
            );
        };
        config.insert("transport".into(), Value::String("resend".into()));
        config.insert("resend_api_key".into(), Value::String(key.into()));
        text(old, "resend_from_email").or_else(|| text(old, "smtp_from_email"))
    } else {
        let Some(host) = text(old, "smtp_host") else {
            return Old::Broken("в старой панели не задан сервер SMTP".into());
        };
        let secure = text(old, "smtp_secure").is_some_and(truthy);
        let port = text(old, "smtp_port")
            .and_then(|p| p.parse::<u16>().ok())
            .filter(|p| *p > 0)
            .unwrap_or(587);
        config.insert("transport".into(), Value::String("smtp".into()));
        config.insert("smtp_host".into(), Value::String(clean_host(host)));
        config.insert(
            "security".into(),
            Value::String(if secure { "tls" } else { "starttls" }.into()),
        );
        config.insert("smtp_port".into(), Value::String(port.to_string()));
        let password = old.get("smtp_password").filter(|p| !p.trim().is_empty());
        if let (Some(user), Some(password)) = (text(old, "smtp_user"), password) {
            config.insert("username".into(), Value::String(user.into()));
            config.insert("password".into(), Value::String(password.clone()));
        }
        text(old, "smtp_from_email")
    };
    let Some(from) = from else {
        return Old::Broken("в старой панели не задан адрес отправителя".into());
    };
    let Ok(from) = normalize_identity(IdentityKind::Email, from) else {
        return Old::Broken(format!(
            "адрес отправителя «{from}» в старой панели записан неверно"
        ));
    };
    config.insert("from_email".into(), Value::String(from));
    if let Some(name) = text(old, "smtp_from_name").and_then(sender_name) {
        config.insert("from_name".into(), Value::String(name));
    }
    match MailSettings::parse(&ModuleConfig(config.clone())) {
        Ok(settings) => Old::Ready(OldMail {
            summary: summary(&settings),
            fingerprint: fingerprint(&settings),
            resend,
            config,
        }),
        Err(reason) => Old::Broken(format!("в старой панели {reason}")),
    }
}

struct Current {
    summary: String,
    fingerprint: Option<String>,
}

fn current(services: &Services) -> Option<Current> {
    let config = services.modules().config(ModuleId::Mail);
    let transport = config.text("transport").map(str::trim);
    if !matches!(transport, Some("smtp" | "resend")) {
        return None;
    }
    Some(match MailSettings::parse(&config) {
        Ok(settings) => Current {
            summary: summary(&settings),
            fingerprint: Some(fingerprint(&settings)),
        },
        Err(reason) => Current {
            summary: format!("настройки с ошибкой: {reason}"),
            fingerprint: None,
        },
    })
}

pub fn has_login(services: &Services) -> bool {
    services
        .modules()
        .config(ModuleId::Mail)
        .text("login")
        .is_some_and(|v| !v.trim().is_empty())
}

pub async fn moved_before(services: &Services, entity: &str) -> ServiceResult<bool> {
    let mut conn = services.store().conn().await?;
    let moved: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM legacy_ids WHERE entity = $1)")
            .bind(entity)
            .fetch_one(sn_store::Db::conn(&mut conn))
            .await?;
    Ok(moved)
}

pub async fn plan(
    services: &Services,
    entity: &str,
    old: &BTreeMap<String, String>,
) -> ServiceResult<Option<MailPlan>> {
    let old = match old_mail(old) {
        Old::Absent => return Ok(None),
        Old::Broken(reason) => return Ok(Some(MailPlan::Skipped { reason })),
        Old::Ready(old) => old,
    };
    let incoming = old.summary;
    Ok(Some(match current(services) {
        Some(now) if now.fingerprint.as_deref() == Some(old.fingerprint.as_str()) => {
            MailPlan::Same { incoming }
        }
        Some(now) => MailPlan::Differs {
            current: now.summary,
            incoming,
        },
        None if moved_before(services, entity).await? => MailPlan::Differs {
            current: "отправка писем выключена".into(),
            incoming,
        },
        None => MailPlan::Transfer { incoming },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn ready(old: Old) -> OldMail {
        match old {
            Old::Ready(mail) => mail,
            other => panic!("ожидались настройки почты: {other:?}"),
        }
    }

    #[test]
    fn smtp_старой_панели_переводится_в_модуль_почты() {
        let mail = ready(old_mail(&settings(&[
            ("mail_provider", "smtp"),
            ("smtp_host", " smtps://Smtp.Example.com:465/ "),
            ("smtp_port", "465"),
            ("smtp_secure", "true"),
            ("smtp_user", "robot@example.com"),
            ("smtp_password", " app pass "),
            ("smtp_from_email", "Robot@Example.com"),
            ("smtp_from_name", "Сервис \"VPN\" <main>"),
        ])));
        assert!(!mail.resend);
        assert_eq!(mail.config["transport"], "smtp");
        assert_eq!(mail.config["smtp_host"], "Smtp.Example.com");
        assert_eq!(mail.config["security"], "tls");
        assert_eq!(mail.config["smtp_port"], "465");
        assert_eq!(mail.config["username"], "robot@example.com");
        assert_eq!(mail.config["password"], " app pass ");
        assert_eq!(mail.config["from_email"], "robot@example.com");
        assert_eq!(mail.config["from_name"], "Сервис VPN main");
        assert_eq!(
            mail.summary,
            "SMTP Smtp.Example.com:465 (TLS), отправитель robot@example.com"
        );

        let plain = ready(old_mail(&settings(&[
            ("smtp_host", "mail.example.com"),
            ("smtp_port", ""),
            ("smtp_secure", "false"),
            ("smtp_user", "robot"),
            ("smtp_from_email", "robot@example.com"),
        ])));
        assert_eq!(plain.config["security"], "starttls");
        assert_eq!(plain.config["smtp_port"], "587");
        assert!(!plain.config.contains_key("username"));
        assert!(!plain.config.contains_key("password"));
        assert!(!plain.config.contains_key("from_name"));
    }

    #[test]
    fn resend_берёт_адрес_из_smtp_если_своего_нет() {
        let mail = ready(old_mail(&settings(&[
            ("mail_provider", "resend"),
            ("resend_api_key", "re_test"),
            ("smtp_host", "smtp.example.com"),
            ("smtp_from_email", "robot@example.com"),
        ])));
        assert!(mail.resend);
        assert_eq!(mail.config["transport"], "resend");
        assert_eq!(mail.config["resend_api_key"], "re_test");
        assert_eq!(mail.config["from_email"], "robot@example.com");
        assert!(!mail.config.contains_key("smtp_host"));
        assert_eq!(mail.summary, "Resend, отправитель robot@example.com");
    }

    #[test]
    fn незаданная_и_сломанная_почта_не_переносится() {
        assert_eq!(old_mail(&settings(&[])), Old::Absent);
        assert_eq!(
            old_mail(&settings(&[
                ("mail_provider", "smtp"),
                ("smtp_port", "587")
            ])),
            Old::Absent
        );
        for broken in [
            settings(&[("smtp_from_email", "robot@example.com")]),
            settings(&[("smtp_host", "smtp.example.com")]),
            settings(&[
                ("mail_provider", "resend"),
                ("resend_from_email", "robot@example.com"),
            ]),
            settings(&[
                ("smtp_host", "smtp.example.com"),
                ("smtp_from_email", "не почта"),
            ]),
            settings(&[
                ("smtp_host", "smtp example com"),
                ("smtp_from_email", "robot@example.com"),
            ]),
        ] {
            assert!(matches!(old_mail(&broken), Old::Broken(_)), "{broken:?}");
        }
    }
}
