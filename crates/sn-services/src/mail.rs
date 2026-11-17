use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use lettre::message::header::ContentType;
use lettre::message::{Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use sn_domain::client::{IdentityKind, normalize_identity};
use sn_domain::module::ModuleId;
use sn_modules::catalog::mail::{MailSecurity, MailSettings, MailTransport, SmtpSettings};
use std::time::Duration;

const SEND_TIMEOUT: Duration = Duration::from_secs(20);
const RESEND_API: &str = "https://api.resend.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Letter {
    pub to: String,
    pub subject: String,
    pub text: String,
}

fn rejected(code: &'static str, message: impl Into<String>) -> ServiceError {
    ServiceError::rejected(code, message)
}

fn address(raw: &str) -> ServiceResult<Address> {
    let email = normalize_identity(IdentityKind::Email, raw)
        .map_err(|_| rejected("mail.address", "Адрес почты записан неверно"))?;
    email
        .parse::<Address>()
        .map_err(|_| rejected("mail.address", "Адрес почты записан неверно"))
}

fn transport(settings: &SmtpSettings) -> ServiceResult<AsyncSmtpTransport<Tokio1Executor>> {
    let builder = match settings.security {
        MailSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&settings.host),
        MailSecurity::StartTls => {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&settings.host)
        }
        MailSecurity::Plain => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
            &settings.host,
        )),
    }
    .map_err(|err| rejected("mail.settings", format!("Сервер SMTP не подходит: {err}")))?;
    let mut builder = builder.port(settings.port).timeout(Some(SEND_TIMEOUT));
    if let (Some(user), Some(password)) = (&settings.username, &settings.password) {
        builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
    }
    Ok(builder.build())
}

pub struct MailOps<'a> {
    services: &'a Services,
    resend_base: String,
}

impl<'a> MailOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        MailOps {
            services,
            resend_base: RESEND_API.to_string(),
        }
    }

    pub fn with_resend_base(mut self, base: impl Into<String>) -> Self {
        self.resend_base = base.into();
        self
    }

    pub fn settings(&self) -> Option<MailSettings> {
        let modules = self.services.modules();
        if !modules.is_enabled(ModuleId::Mail) {
            return None;
        }
        MailSettings::parse(&modules.config(ModuleId::Mail)).ok()
    }

    pub fn available(&self) -> bool {
        self.settings().is_some_and(|s| s.sends())
    }

    fn sender_name(&self, settings: &MailSettings) -> String {
        settings
            .from_name
            .clone()
            .unwrap_or_else(|| self.services.settings().brand_name.clone())
    }

    pub async fn send(&self, letter: &Letter) -> ServiceResult<()> {
        let settings = self.settings().filter(|s| s.sends()).ok_or_else(|| {
            rejected(
                "mail.unavailable",
                "Письма не отправляются: в «Почте» выбран вход без подтверждения",
            )
        })?;
        let to = address(&letter.to)?;
        let name = self.sender_name(&settings);
        match &settings.transport {
            MailTransport::Smtp(smtp) => self.send_smtp(smtp, &settings, &name, to, letter).await,
            MailTransport::Resend { api_key } => {
                self.send_resend(api_key, &settings, &name, to, letter)
                    .await
            }
            MailTransport::Off => Err(rejected(
                "mail.unavailable",
                "Письма не отправляются: в «Почте» выбран вход без подтверждения",
            )),
        }
    }

    async fn send_smtp(
        &self,
        smtp: &SmtpSettings,
        settings: &MailSettings,
        name: &str,
        to: Address,
        letter: &Letter,
    ) -> ServiceResult<()> {
        let message = Message::builder()
            .from(Mailbox::new(
                Some(name.to_string()),
                address(&settings.from_email)?,
            ))
            .to(Mailbox::new(None, to))
            .subject(letter.subject.clone())
            .header(ContentType::TEXT_PLAIN)
            .body(letter.text.clone())
            .map_err(|err| rejected("mail.letter", format!("Письмо не собралось: {err}")))?;
        let transport = transport(smtp)?;
        match tokio::time::timeout(SEND_TIMEOUT, transport.send(message)).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(err)) => {
                tracing::warn!(error = %err, host = %smtp.host, "письмо не отправилось");
                Err(rejected(
                    "mail.failed",
                    format!("Письмо не отправилось: {err}"),
                ))
            }
            Err(_) => Err(rejected(
                "mail.failed",
                "Почтовый сервер не ответил за 20 секунд",
            )),
        }
    }

    async fn send_resend(
        &self,
        api_key: &str,
        settings: &MailSettings,
        name: &str,
        to: Address,
        letter: &Letter,
    ) -> ServiceResult<()> {
        let client = reqwest::Client::builder()
            .timeout(SEND_TIMEOUT)
            .build()
            .map_err(|err| rejected("mail.failed", format!("Письмо не отправилось: {err}")))?;
        let body = serde_json::json!({
            "from": format!("{name} <{}>", settings.from_email),
            "to": [to.to_string()],
            "subject": letter.subject,
            "text": letter.text,
        });
        let response = client
            .post(format!("{}/emails", self.resend_base.trim_end_matches('/')))
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .map_err(|err| {
                tracing::warn!(error = %err.without_url(), "Resend не ответил");
                rejected("mail.failed", "Resend не ответил, попробуйте позже")
            })?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let message = response
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        tracing::warn!(status = status.as_u16(), %message, "Resend отказал");
        Err(rejected(
            "mail.failed",
            format!("Resend отказал ({}): {message}", status.as_u16()),
        ))
    }

    pub async fn test(&self, to: &str) -> ServiceResult<()> {
        let brand = self.services.settings().brand_name.clone();
        self.send(&Letter {
            to: to.to_string(),
            subject: format!("Проверка почты {brand}"),
            text: format!(
                "Это проверочное письмо из админки {brand}.\n\nЕсли оно дошло, почта настроена правильно: коды входа в кабинет будут приходить с этого адреса."
            ),
        })
        .await
    }
}
