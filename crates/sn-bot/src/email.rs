use crate::BotError;
use crate::handler::{Screen, Turn};
use crate::session::Awaiting;
use crate::telegram::{Button, Keyboard};
use crate::text;
use crate::ui::{self, Lines, Text};
use sn_domain::access::{ClientChannel, ClientPrincipal};
use sn_domain::bot_content::Section;
use sn_domain::client::{Client, IdentityKind, normalize_identity};
use sn_domain::format;
use sn_domain::lang;
use sn_services::ServiceError;
use sn_services::Services;
use sn_services::auth::RequestMeta;
use sn_services::bot_content::BotContent;
use sn_services::email_change::{EmailProof, EmailProofInput};
use sn_services::login::EMAIL_CODE_TTL_SECONDS;

pub const CALLBACK: &str = "em";

pub fn cabinet_url(services: &Services) -> Option<String> {
    let settings = services.settings();
    let base = settings.public_url.as_deref()?.trim().trim_end_matches('/');
    (base.starts_with("https://") || base.starts_with("http://")).then(|| format!("{base}/cabinet"))
}

pub fn offered(services: &Services) -> bool {
    sn_services::bot_layout::email_offered(services)
}

fn by_password(services: &Services) -> bool {
    !services.mail().code_login()
}

fn principal(client: &Client) -> ClientPrincipal {
    ClientPrincipal {
        client_id: client.id,
        channel: ClientChannel::Bot,
        full_access: true,
        session_id: None,
    }
}

fn minutes() -> String {
    let minutes = u64::try_from(EMAIL_CODE_TTL_SECONDS / 60).unwrap_or(15);
    format::count_in(
        minutes,
        lang::current(),
        ["минуту", "минуты", "минут"],
        ["minute", "minutes"],
    )
}

fn screen(lines: Lines, keyboard: Keyboard) -> Screen {
    Screen::new(lines.text())
        .keyboard(keyboard)
        .section(Section::Login)
}

fn error_line(c: &BotContent, err: &ServiceError) -> String {
    ui::text(c, "email.error", &[("error", Text(&text::user_error(err)))])
}

fn how_to(c: &BotContent, services: &Services, email: &str) -> String {
    let url = cabinet_url(services).unwrap_or_default();
    let key = if by_password(services) {
        "email.how_password"
    } else {
        "email.how_code"
    };
    ui::text(c, key, &[("url", Text(&url)), ("email", Text(email))])
}

fn password_button(c: &BotContent, services: &Services) -> Option<Button> {
    match services.settings().miniapp_url() {
        Some(app) => ui::web_app(c, "email.btn.password", format!("{app}/profile/password")),
        None => ui::button(c, "email.btn.password", &[], format!("{CALLBACK}:pw")),
    }
}

fn off_screen(c: &BotContent) -> Screen {
    let mut lines = Lines::new();
    lines.add(ui::text(c, "email.off", &[]));
    screen(lines, Keyboard::new().push(ui::menu(c)))
}

fn code_keyboard(c: &BotContent) -> Keyboard {
    Keyboard::new()
        .push(ui::button(
            c,
            "email.btn.resend",
            &[],
            format!("{CALLBACK}:again"),
        ))
        .push(ui::button(
            c,
            "email.btn.other",
            &[],
            format!("{CALLBACK}:new"),
        ))
        .push(ui::cancel(c, CALLBACK))
}

fn proof_line(c: &BotContent, proof: EmailProof, email: &str) -> String {
    let key = match proof {
        EmailProof::Totp => "email.ask_app_code",
        EmailProof::Password => "email.ask_password",
    };
    ui::text(c, key, &[("email", Text(email))])
}

fn password_prompt(c: &BotContent, services: &Services) -> String {
    let url = cabinet_url(services).unwrap_or_default();
    ui::text(c, "email.set_password_chat", &[("url", Text(&url))])
}

pub async fn action(turn: &mut Turn<'_>, client: &Client, rest: &str) -> Result<Screen, BotError> {
    match rest {
        "new" => ask_address(turn, client).await,
        "again" => resend(turn, client).await,
        "pw" => ask_password(turn, client).await,
        _ => open(turn, client).await,
    }
}

pub async fn open(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    turn.session.email = None;
    let services = turn.services().clone();
    let content = turn.content();
    let c = &*content;
    if !offered(&services) {
        return Ok(off_screen(c));
    }
    let state = services.auth().email_state(client.id).await?;
    let mut lines = Lines::new();
    lines.add(ui::text(c, "email.title", &[]));
    let mut keyboard = Keyboard::new();
    match &state.email {
        Some(email) => {
            lines.add(ui::text(c, "email.current", &[("email", Text(email))]));
            let password = by_password(&services);
            let has_password = password && services.auth().has_password(client.id).await?;
            if password {
                lines.add(ui::text(
                    c,
                    if has_password {
                        "email.password_yes"
                    } else {
                        "email.password_no"
                    },
                    &[],
                ));
            }
            lines.blank();
            lines.add(how_to(c, &services, email));
            if password && !has_password {
                keyboard = keyboard.push(password_button(c, &services));
            }
            keyboard = keyboard.push(ui::button(
                c,
                "email.btn.change",
                &[],
                format!("{CALLBACK}:new"),
            ));
        }
        None => {
            lines.add(ui::text(c, "email.none", &[]));
            keyboard = keyboard.push(ui::button(
                c,
                "email.btn.link",
                &[],
                format!("{CALLBACK}:new"),
            ));
        }
    }
    Ok(screen(lines, keyboard.push(ui::menu(c))))
}

async fn ask_address(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = turn.content();
    let c = &*content;
    turn.session.email = None;
    if !offered(&services) {
        return Ok(off_screen(c));
    }
    let state = services.auth().email_state(client.id).await?;
    if state.proof == Some(EmailProof::Password)
        && let Some(app) = services.settings().miniapp_url()
    {
        let mut lines = Lines::new();
        lines.add(ui::text(c, "email.change_in_app", &[]));
        return Ok(screen(
            lines,
            Keyboard::new()
                .push(ui::web_app(
                    c,
                    "email.btn.change_app",
                    format!("{app}/profile/email"),
                ))
                .push(ui::back(c, CALLBACK)),
        ));
    }
    turn.session.wait_for(Awaiting::EmailAddress);
    let mut lines = Lines::new();
    lines.add(ui::text(c, "email.ask", &[]));
    Ok(screen(lines, Keyboard::new().push(ui::cancel(c, CALLBACK))))
}

pub async fn address_entered(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = turn.content();
    let c = &*content;
    if !offered(&services) {
        return Ok(off_screen(c));
    }
    let Ok(address) = normalize_identity(IdentityKind::Email, raw) else {
        turn.session.wait_for(Awaiting::EmailAddress);
        let mut lines = Lines::new();
        lines.add(ui::text(c, "email.invalid", &[]));
        return Ok(screen(lines, Keyboard::new().push(ui::cancel(c, CALLBACK))));
    };
    let state = services.auth().email_state(client.id).await?;
    match state.proof {
        Some(proof) => {
            turn.session.email = Some(address.clone());
            turn.session.wait_for(Awaiting::EmailProof);
            let mut lines = Lines::new();
            lines.add(proof_line(c, proof, &address));
            Ok(screen(lines, Keyboard::new().push(ui::cancel(c, CALLBACK))))
        }
        None => send_code(turn, client, &address, None).await,
    }
}

pub async fn proof_entered(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    let Some(address) = turn.session.email.clone() else {
        return open(turn, client).await;
    };
    let services = turn.services().clone();
    let state = services.auth().email_state(client.id).await?;
    match state.proof {
        Some(proof) => send_code(turn, client, &address, Some((proof, raw))).await,
        None => send_code(turn, client, &address, None).await,
    }
}

async fn send_code(
    turn: &mut Turn<'_>,
    client: &Client,
    address: &str,
    proof: Option<(EmailProof, &str)>,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = turn.content();
    let c = &*content;
    let input = match proof {
        Some((EmailProof::Totp, code)) => EmailProofInput {
            password: None,
            code: Some(code),
        },
        Some((EmailProof::Password, password)) => EmailProofInput {
            password: Some(password),
            code: None,
        },
        None => EmailProofInput::default(),
    };
    let sent = services
        .auth()
        .email_change_start(&principal(client), address, input, &RequestMeta::default())
        .await;
    let mut lines = Lines::new();
    if turn.secret_left {
        lines.add(ui::text(c, "email.secret_left", &[]));
    }
    match sent {
        Ok(pending) => {
            turn.session.email = Some(pending.email.clone());
            turn.session.wait_for(Awaiting::EmailCode);
            lines.add(ui::text(
                c,
                "email.code_sent",
                &[
                    ("email", Text(&pending.email)),
                    ("minutes", Text(&minutes())),
                ],
            ));
            Ok(screen(lines, code_keyboard(c)))
        }
        Err(err) => {
            lines.add(error_line(c, &err));
            match proof {
                Some((kind, _)) => {
                    turn.session.email = Some(address.to_string());
                    turn.session.wait_for(Awaiting::EmailProof);
                    lines.add(proof_line(c, kind, address));
                    Ok(screen(
                        lines,
                        Keyboard::new()
                            .push(ui::button(
                                c,
                                "email.btn.other",
                                &[],
                                format!("{CALLBACK}:new"),
                            ))
                            .push(ui::cancel(c, CALLBACK)),
                    ))
                }
                None => {
                    turn.session.email = None;
                    turn.session.wait_for(Awaiting::EmailAddress);
                    lines.add(ui::text(c, "email.ask", &[]));
                    Ok(screen(lines, Keyboard::new().push(ui::cancel(c, CALLBACK))))
                }
            }
        }
    }
}

async fn resend(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let Some(address) = turn.session.email.clone() else {
        return ask_address(turn, client).await;
    };
    let services = turn.services().clone();
    let content = turn.content();
    let c = &*content;
    if !offered(&services) {
        return Ok(off_screen(c));
    }
    let state = services.auth().email_state(client.id).await?;
    match state.proof {
        Some(proof) => {
            turn.session.wait_for(Awaiting::EmailProof);
            let mut lines = Lines::new();
            lines.add(proof_line(c, proof, &address));
            Ok(screen(lines, Keyboard::new().push(ui::cancel(c, CALLBACK))))
        }
        None => send_code(turn, client, &address, None).await,
    }
}

pub async fn code_entered(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    let Some(address) = turn.session.email.clone() else {
        return open(turn, client).await;
    };
    let services = turn.services().clone();
    let content = turn.content();
    let c = &*content;
    let confirmed = services
        .auth()
        .email_change_confirm(&principal(client), &address, raw, &RequestMeta::default())
        .await;
    match confirmed {
        Ok(state) => {
            turn.session.email = None;
            linked(turn, client, &state.email.unwrap_or(address)).await
        }
        Err(err) => {
            turn.session.wait_for(Awaiting::EmailCode);
            let mut lines = Lines::new();
            lines.add(error_line(c, &err));
            lines.add(ui::text(
                c,
                "email.code_again",
                &[("email", Text(&address))],
            ));
            Ok(screen(lines, code_keyboard(c)))
        }
    }
}

async fn linked(turn: &mut Turn<'_>, client: &Client, email: &str) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = turn.content();
    let c = &*content;
    let mut lines = Lines::new();
    lines.add(ui::text(c, "email.linked", &[("email", Text(email))]));
    let password = by_password(&services);
    let has_password = password && services.auth().has_password(client.id).await?;
    let mut keyboard = Keyboard::new();
    if password && !has_password {
        match services.settings().miniapp_url() {
            Some(app) => {
                lines.add(ui::text(c, "email.set_password_app", &[]));
                lines.blank();
                lines.add(how_to(c, &services, email));
                keyboard = keyboard.push(ui::web_app(
                    c,
                    "email.btn.password",
                    format!("{app}/profile/password"),
                ));
            }
            None => {
                turn.session.wait_for(Awaiting::SitePassword);
                lines.add(password_prompt(c, &services));
            }
        }
    } else {
        lines.blank();
        lines.add(how_to(c, &services, email));
    }
    Ok(screen(lines, keyboard.push(ui::menu(c))))
}

async fn ask_password(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = turn.content();
    let c = &*content;
    if !offered(&services) || !by_password(&services) {
        return open(turn, client).await;
    }
    let state = services.auth().email_state(client.id).await?;
    if state.email.is_none() || services.auth().has_password(client.id).await? {
        return open(turn, client).await;
    }
    turn.session.wait_for(Awaiting::SitePassword);
    let mut lines = Lines::new();
    lines.add(password_prompt(c, &services));
    Ok(screen(lines, Keyboard::new().push(ui::menu(c))))
}

pub async fn password_entered(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = turn.content();
    let c = &*content;
    let mut lines = Lines::new();
    if turn.secret_left {
        lines.add(ui::text(c, "email.secret_left", &[]));
    }
    let state = services.auth().email_state(client.id).await?;
    let Some(email) = state.email else {
        lines.add(ui::text(c, "email.none", &[]));
        return Ok(screen(
            lines,
            Keyboard::new()
                .push(ui::button(
                    c,
                    "email.btn.link",
                    &[],
                    format!("{CALLBACK}:new"),
                ))
                .push(ui::menu(c)),
        ));
    };
    if services.auth().has_password(client.id).await? {
        lines.add(ui::text(c, "email.password_yes", &[]));
        lines.blank();
        lines.add(how_to(c, &services, &email));
        return Ok(screen(lines, Keyboard::new().push(ui::menu(c))));
    }
    match services
        .auth()
        .change_password(&principal(client), None, raw)
        .await
    {
        Ok(()) => {
            lines.add(ui::text(c, "email.password_saved", &[]));
            lines.blank();
            lines.add(how_to(c, &services, &email));
            Ok(screen(lines, Keyboard::new().push(ui::menu(c))))
        }
        Err(err) => {
            lines.add(error_line(c, &err));
            if err.code() == "auth.weak_password" {
                turn.session.wait_for(Awaiting::SitePassword);
                lines.add(password_prompt(c, &services));
            }
            Ok(screen(lines, Keyboard::new().push(ui::menu(c))))
        }
    }
}
