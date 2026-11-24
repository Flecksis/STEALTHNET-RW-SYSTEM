use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use async_trait::async_trait;
use lettre::message::header::{ContentType, HeaderName, HeaderValue};
use lettre::message::{Mailbox, Message, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use sn_domain::client::{IdentityKind, normalize_identity};
use sn_domain::module::ModuleId;
use sn_modules::catalog::mail::{
    ClientLetters, EmailLogin, MailSecurity, MailSettings, MailTransport, SmtpSettings,
};
use sn_modules::{MailDelivery, MailOut, MailSender, MailSite};
use std::time::Duration;

const SEND_TIMEOUT: Duration = Duration::from_secs(20);
pub(crate) const RESEND_API: &str = "https://api.resend.com";

fn resend_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().timeout(SEND_TIMEOUT)
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichLetter {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: String,
    pub unsubscribe: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Undelivered {
    Off(String),
    Address(String),
    Limited(Duration),
    Failed(String),
}

const UNSUBSCRIBE_POST: &str = "List-Unsubscribe=One-Click";

fn smtp_reply(err: &lettre::transport::smtp::Error) -> String {
    let text = err.to_string();
    for prefix in ["permanent error (", "transient error ("] {
        if let Some(rest) = text.strip_prefix(prefix)
            && let Some((code, reply)) = rest.split_once("): ")
        {
            return format!("код {code}: {reply}");
        }
    }
    text
}

fn smtp_failure(err: &lettre::transport::smtp::Error) -> Undelivered {
    let code = err.status().map(|c| c.to_string());
    if err.is_permanent() {
        match code.as_deref() {
            Some("530" | "534" | "535" | "538") => Undelivered::Off(format!(
                "почтовый сервер не принял логин и пароль ({})",
                code.unwrap_or_default()
            )),
            _ => Undelivered::Address(format!("Почтовый сервер отказал, {}", smtp_reply(err))),
        }
    } else if err.is_transient() {
        Undelivered::Failed(format!(
            "Почтовый сервер попросил повторить позже, {}",
            smtp_reply(err)
        ))
    } else {
        Undelivered::Off(format!("нет связи с почтовым сервером: {err}"))
    }
}

fn retry_after(response: &reqwest::Response) -> Duration {
    let seconds = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(1);
    Duration::from_secs(seconds.clamp(1, 600))
}

pub struct MailOps<'a> {
    services: &'a Services,
    resend_base: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignupRule {
    Open,
    Confirm,
    Closed,
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

    fn resend_client(&self) -> reqwest::Client {
        let proxy = self
            .services
            .egress()
            .proxy_for(sn_egress::Route::Mail, None);
        sn_egress::build(resend_builder(), proxy.as_deref())
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

    pub fn via_resend(&self) -> bool {
        self.settings()
            .is_some_and(|s| matches!(s.transport, MailTransport::Resend { .. }))
    }

    pub fn code_login(&self) -> bool {
        self.settings().is_some_and(|s| s.code_login())
    }

    pub fn codes_per_hour(&self) -> u32 {
        self.settings()
            .map_or(sn_modules::catalog::mail::CODES_PER_HOUR_DEFAULT, |s| {
                s.codes_per_hour
            })
    }

    pub fn signup_rule(&self) -> SignupRule {
        let asked = MailSettings::parse(&self.services.modules().config(ModuleId::Mail))
            .is_ok_and(|s| s.confirm_signup && s.login == EmailLogin::Password);
        if !asked {
            SignupRule::Open
        } else if self.available() {
            SignupRule::Confirm
        } else {
            SignupRule::Closed
        }
    }

    pub fn letters(&self) -> Option<ClientLetters> {
        self.settings().filter(|s| s.sends()).map(|s| s.letters)
    }

    pub fn readiness(&self) -> Result<MailSettings, String> {
        let modules = self.services.modules();
        if !modules.is_enabled(ModuleId::Mail) {
            return Err("модуль «Почта» выключен".into());
        }
        let settings = MailSettings::parse(&modules.config(ModuleId::Mail))
            .map_err(|reason| format!("настройки почты неполные: {reason}"))?;
        if !settings.sends() {
            return Err("в «Почте» не выбран способ отправки писем".into());
        }
        Ok(settings)
    }

    pub async fn deliver(&self, letter: &RichLetter) -> Result<(), Undelivered> {
        let settings = self.readiness().map_err(Undelivered::Off)?;
        let to = address(&letter.to)
            .map_err(|_| Undelivered::Address("Адрес почты записан неверно".into()))?;
        let name = self.sender_name(&settings);
        match &settings.transport {
            MailTransport::Smtp(smtp) => {
                self.deliver_smtp(smtp, &settings, &name, to, letter).await
            }
            MailTransport::Resend { api_key } => {
                self.deliver_resend(api_key, &settings, &name, to, letter)
                    .await
            }
            MailTransport::Off => Err(Undelivered::Off(
                "в «Почте» не выбран способ отправки писем".into(),
            )),
        }
    }

    async fn deliver_smtp(
        &self,
        smtp: &SmtpSettings,
        settings: &MailSettings,
        name: &str,
        to: Address,
        letter: &RichLetter,
    ) -> Result<(), Undelivered> {
        let from = address(&settings.from_email)
            .map_err(|_| Undelivered::Off("адрес отправителя записан неверно".into()))?;
        let mut builder = Message::builder()
            .from(Mailbox::new(Some(name.to_string()), from))
            .to(Mailbox::new(None, to))
            .subject(letter.subject.clone());
        if let Some(url) = &letter.unsubscribe {
            builder = builder
                .raw_header(HeaderValue::new(
                    HeaderName::new_from_ascii_str("List-Unsubscribe"),
                    format!("<{url}>"),
                ))
                .raw_header(HeaderValue::new(
                    HeaderName::new_from_ascii_str("List-Unsubscribe-Post"),
                    UNSUBSCRIBE_POST.to_string(),
                ));
        }
        let message = builder
            .multipart(MultiPart::alternative_plain_html(
                letter.text.clone(),
                letter.html.clone(),
            ))
            .map_err(|err| Undelivered::Failed(format!("Письмо не собралось: {err}")))?;
        let transport = transport(smtp).map_err(|err| Undelivered::Off(err.to_string()))?;
        match tokio::time::timeout(SEND_TIMEOUT, transport.send(message)).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(err)) => {
                tracing::debug!(error = %err, host = %smtp.host, "письмо рассылки не отправилось");
                Err(smtp_failure(&err))
            }
            Err(_) => Err(Undelivered::Failed(
                "Почтовый сервер не ответил за 20 секунд".into(),
            )),
        }
    }

    async fn deliver_resend(
        &self,
        api_key: &str,
        settings: &MailSettings,
        name: &str,
        to: Address,
        letter: &RichLetter,
    ) -> Result<(), Undelivered> {
        let client = self.resend_client();
        let mut body = serde_json::json!({
            "from": format!("{name} <{}>", settings.from_email),
            "to": [to.to_string()],
            "subject": letter.subject,
            "text": letter.text,
            "html": letter.html,
        });
        if let Some(url) = &letter.unsubscribe {
            body["headers"] = serde_json::json!({
                "List-Unsubscribe": format!("<{url}>"),
                "List-Unsubscribe-Post": UNSUBSCRIBE_POST,
            });
        }
        let response = client
            .post(format!("{}/emails", self.resend_base.trim_end_matches('/')))
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .map_err(|err| {
                tracing::debug!(error = %err.without_url(), "Resend не ответил");
                Undelivered::Off("Resend не отвечает".into())
            })?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        if status.as_u16() == 429 {
            return Err(Undelivered::Limited(retry_after(&response)));
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
        let code = status.as_u16();
        let text = format!("Resend отказал ({code}): {message}");
        Err(match code {
            401 | 403 => Undelivered::Off(text),
            400 | 422 if message.contains("`to`") => Undelivered::Address(text),
            400 | 422 => Undelivered::Off(text),
            _ if status.is_server_error() => {
                Undelivered::Failed(format!("Resend не ответил ({code})"))
            }
            _ => Undelivered::Address(text),
        })
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
        let client = self.resend_client();
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

pub struct MailBridge {
    services: Services,
    resend_base: Option<String>,
}

impl MailBridge {
    pub fn new(services: Services) -> Self {
        MailBridge {
            services,
            resend_base: None,
        }
    }

    pub fn with_resend_base(mut self, base: impl Into<String>) -> Self {
        self.resend_base = Some(base.into());
        self
    }

    fn ops(&self) -> MailOps<'_> {
        let ops = MailOps::new(&self.services);
        match &self.resend_base {
            Some(base) => ops.with_resend_base(base.clone()),
            None => ops,
        }
    }
}

#[async_trait]
impl MailSender for MailBridge {
    fn site(&self) -> Result<MailSite, String> {
        self.ops().readiness()?;
        let core = self.services.settings();
        let base = core
            .public_url
            .as_deref()
            .map(|u| u.trim().trim_end_matches('/'))
            .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
            .ok_or_else(|| sn_modules::sender::NO_SITE_ADDRESS.to_string())?;
        Ok(MailSite {
            base_url: base.to_string(),
            brand: core.brand_name.clone(),
        })
    }

    async fn send(&self, mail: &MailOut<'_>) -> MailDelivery {
        let letter = RichLetter {
            to: mail.to.to_string(),
            subject: mail.subject.to_string(),
            text: mail.text.to_string(),
            html: mail.html.to_string(),
            unsubscribe: mail.unsubscribe.map(str::to_string),
        };
        match self.ops().deliver(&letter).await {
            Ok(()) => MailDelivery::Sent,
            Err(Undelivered::Off(reason)) => MailDelivery::Off(reason),
            Err(Undelivered::Address(reason)) => MailDelivery::Rejected(reason),
            Err(Undelivered::Limited(pause)) => MailDelivery::Wait(pause),
            Err(Undelivered::Failed(error)) => MailDelivery::Failed { error, retry: true },
        }
    }
}
