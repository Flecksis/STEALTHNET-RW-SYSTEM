use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use lettre::message::header::ContentType;
use lettre::message::{Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use sn_domain::client::{IdentityKind, normalize_identity};
use sn_domain::module::ModuleId;
use sn_modules::catalog::mail::{MailSecurity, MailSettings};
use std::time::Duration;

const SEND_TIMEOUT: Duration = Duration::from_secs(20);

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

fn transport(settings: &MailSettings) -> ServiceResult<AsyncSmtpTransport<Tokio1Executor>> {
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
}

impl<'a> MailOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        MailOps { services }
    }

    pub fn settings(&self) -> Option<MailSettings> {
        let modules = self.services.modules();
        if !modules.is_enabled(ModuleId::Mail) {
            return None;
        }
        MailSettings::parse(&modules.config(ModuleId::Mail)).ok()
    }

    pub fn available(&self) -> bool {
        self.settings().is_some()
    }

    pub async fn send(&self, letter: &Letter) -> ServiceResult<()> {
        let settings = self.settings().ok_or_else(|| {
            rejected(
                "mail.unavailable",
                "Почта не настроена: включите модуль «Почта» и заполните подключение к SMTP",
            )
        })?;
        let name = settings
            .from_name
            .clone()
            .unwrap_or_else(|| self.services.settings().brand_name.clone());
        let message = Message::builder()
            .from(Mailbox::new(Some(name), address(&settings.from_email)?))
            .to(Mailbox::new(None, address(&letter.to)?))
            .subject(letter.subject.clone())
            .header(ContentType::TEXT_PLAIN)
            .body(letter.text.clone())
            .map_err(|err| rejected("mail.letter", format!("Письмо не собралось: {err}")))?;
        let transport = transport(&settings)?;
        match tokio::time::timeout(SEND_TIMEOUT, transport.send(message)).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(err)) => {
                tracing::warn!(error = %err, host = %settings.host, "письмо не отправилось");
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
