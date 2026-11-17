use crate::session::{Awaiting, Session};
use crate::stars::{self, Settled};
use crate::telegram::{
    BotUser, CallbackQuery, Keyboard, Message, PreCheckoutQuery, TelegramApi, Update,
};
use crate::text::{self, escape};
use crate::ui::{self, Html, Lines, Text};
use crate::{BotError, account, buy, consent, extras, gram_wallet, login, media};
use sn_domain::bot_content::Section;
use sn_domain::client::{Client, IdentityKind, RegistrationSource, TelegramProfile};
use sn_domain::ids::TariffId;
use sn_domain::ledger::Wallet;
use sn_domain::subscription::Subscription;
use sn_modules::catalog::{options, promo, referral};
use sn_payments::providers::{StarsPayment, StarsPaymentEvent, StarsPreCheckout};
use sn_services::Services;
use sn_services::bot_content::{BotContent, Document};
use sn_services::subscriptions::SubscriptionView;
use sn_store::Db;
use std::sync::Arc;

#[derive(Clone)]
pub struct BotCtx {
    pub services: Services,
    pub api: Arc<TelegramApi>,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Screen {
    pub text: String,
    pub keyboard: Keyboard,
    pub section: Option<Section>,
    pub menu: bool,
}

impl Screen {
    pub fn new(text: impl Into<String>) -> Self {
        Screen {
            text: text.into(),
            keyboard: Keyboard::new(),
            section: None,
            menu: false,
        }
    }

    pub fn keyboard(mut self, keyboard: Keyboard) -> Self {
        self.keyboard = keyboard;
        self
    }

    pub fn notice(text: impl Into<String>) -> Self {
        Screen::new(text).with_menu()
    }

    pub fn with_menu(mut self) -> Self {
        self.menu = true;
        self
    }

    pub fn section(mut self, section: Section) -> Self {
        self.section = Some(section);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub chat_id: i64,
    pub message_id: Option<i64>,
    pub photo: bool,
}

impl BotCtx {
    pub async fn show(&self, target: Target, screen: &Screen) -> Result<(), BotError> {
        media::show(self, target, screen).await
    }
}

pub fn profile(user: &BotUser) -> TelegramProfile {
    TelegramProfile {
        id: user.id,
        username: user.username.clone(),
        first_name: Some(user.first_name.clone()).filter(|n| !n.trim().is_empty()),
        last_name: user.last_name.clone(),
        language_code: user.language_code.clone(),
        is_premium: user.is_premium.unwrap_or(false),
    }
}

pub struct Turn<'a> {
    pub ctx: &'a BotCtx,
    pub user: &'a BotUser,
    pub target: Target,
    pub session: Session,
}

impl Turn<'_> {
    pub fn services(&self) -> &Services {
        &self.ctx.services
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Start(Option<String>),
    Menu,
    PaySupport,
    Input(Awaiting, String),
    Text(String),
    Callback(String),
}

enum Gate {
    Ready(Client),
    Stop(Screen),
}

pub fn parse_command(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let rest = text.strip_prefix('/')?;
    let (head, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let command = head.split('@').next().unwrap_or(head).to_ascii_lowercase();
    if command.is_empty() {
        return None;
    }
    Some((command, args.trim().to_string()))
}

pub async fn handle_update(ctx: &BotCtx, update: Update) {
    let id = update.update_id;
    if let Err(err) = dispatch(ctx, update).await {
        tracing::warn!(update_id = id, error = %err, "апдейт бота не обработан");
    }
}

async fn dispatch(ctx: &BotCtx, update: Update) -> Result<(), BotError> {
    if let Some(query) = update.widget_query {
        return gram_wallet::on_query(ctx, query).await;
    }
    if let Some(query) = update.pre_checkout_query {
        return pre_checkout(ctx, &query).await;
    }
    if let Some(callback) = update.callback_query {
        return on_callback(ctx, callback).await;
    }
    let Some(message) = update.message else {
        return Ok(());
    };
    if !message.chat.is_private() {
        return Ok(());
    }
    let Some(user) = message.from.clone().filter(|u| !u.is_bot) else {
        return Ok(());
    };
    let target = Target {
        chat_id: message.chat.id,
        message_id: None,
        photo: false,
    };
    if let Some(event) = stars::from_message(&message) {
        return match event.event {
            StarsPaymentEvent::SuccessfulPayment => stars_paid(ctx, target, &event).await,
            StarsPaymentEvent::RefundedPayment => {
                stars_refunded(ctx, &event).await;
                Ok(())
            }
        };
    }
    let Some(text) = message.text.as_deref() else {
        return Ok(());
    };
    if parse_command(text).is_some_and(|(command, _)| command == "emoji") {
        return ctx.show(target, &emoji_codes(&message)).await;
    }
    let session = Session::load(&ctx.services, target.chat_id).await?;
    let action = match parse_command(text) {
        Some((command, args)) => match command.as_str() {
            "start" => Action::Start(Some(args).filter(|a| !a.is_empty())),
            "paysupport" => Action::PaySupport,
            _ => Action::Menu,
        },
        None => match session.awaiting {
            Some(awaiting) => Action::Input(awaiting, text.trim().to_string()),
            None => Action::Text(text.trim().to_string()),
        },
    };
    run(ctx, &user, target, session, action).await
}

async fn on_callback(ctx: &BotCtx, callback: CallbackQuery) -> Result<(), BotError> {
    if let Err(err) = ctx.api.answer_callback(&callback.id, None).await {
        tracing::debug!(error = %err, "ответ на нажатие кнопки не отправлен");
    }
    let Some(message) = callback.message.as_ref().filter(|m| m.chat.is_private()) else {
        return Ok(());
    };
    let target = Target {
        chat_id: message.chat.id,
        message_id: Some(message.message_id),
        photo: message.has_photo(),
    };
    let session = Session::load(&ctx.services, target.chat_id).await?;
    let data = callback.data.clone().unwrap_or_default();
    run(ctx, &callback.from, target, session, Action::Callback(data)).await
}

async fn run(
    ctx: &BotCtx,
    user: &BotUser,
    target: Target,
    session: Session,
    action: Action,
) -> Result<(), BotError> {
    let mut turn = Turn {
        ctx,
        user,
        target,
        session,
    };
    let screen = process(&mut turn, action).await?;
    ctx.show(target, &screen).await?;
    turn.session.save(&ctx.services, target.chat_id).await?;
    Ok(())
}

async fn process(turn: &mut Turn<'_>, action: Action) -> Result<Screen, BotError> {
    let action = match action {
        Action::PaySupport => return Ok(paysupport(turn.ctx)),
        Action::Callback(data) if data == "terms:ok" => {
            accept_documents(turn).await?;
            Action::Start(turn.session.pending_start.clone())
        }
        other => other,
    };
    let start_param = match &action {
        Action::Start(param) => param.clone(),
        _ => None,
    };
    let client = match enter(turn, start_param.as_deref()).await? {
        Gate::Ready(client) => client,
        Gate::Stop(screen) => return Ok(screen),
    };
    match action {
        Action::Start(param) => after_start(turn, &client, param.as_deref()).await,
        Action::Menu | Action::PaySupport => {
            turn.session.awaiting = None;
            main_menu(turn.ctx, &client).await
        }
        Action::Input(awaiting, text) => input(turn, &client, awaiting, &text).await,
        Action::Text(text) => match account::support_followup(turn, &client, &text).await? {
            Some(screen) => Ok(screen),
            None => main_menu(turn.ctx, &client).await,
        },
        Action::Callback(data) => callback(turn, &client, &data).await,
    }
}

pub fn emoji_codes(message: &Message) -> Screen {
    let mut found: Vec<(String, String)> = Vec::new();
    for (fallback, id) in message.custom_emoji() {
        if !found.iter().any(|(_, known)| *known == id) {
            found.push((fallback, id));
        }
    }
    if found.is_empty() {
        return Screen::notice(
            "Пришлите /emoji вместе с премиум-эмодзи в одном сообщении — бот ответит их кодами для текстов и кнопок.",
        );
    }
    let mut out = vec!["<b>Коды премиум-эмодзи</b>".to_string()];
    for (fallback, id) in &found {
        out.push(String::new());
        out.push(format!(
            "{} — <code>{}</code>",
            escape(fallback),
            escape(id)
        ));
        out.push(format!(
            "<code>{}</code>",
            escape(&format!(
                "<tg-emoji emoji-id=\"{id}\">{fallback}</tg-emoji>"
            ))
        ));
    }
    out.push(String::new());
    out.push(
        "Код из первой строки вставьте в поле «Премиум-эмодзи» у кнопки, а строку с тегом — прямо в текст."
            .into(),
    );
    Screen::notice(out.join("\n"))
}

async fn enter(turn: &mut Turn<'_>, start_param: Option<&str>) -> Result<Gate, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content();
    {
        let documents = content.consent_documents();
        let mut conn = services.store().conn().await?;
        if let Some(version) = consent::version(documents)
            && !consent::accepted(&mut conn, turn.user.id, &version).await?
        {
            if let Some(param) = start_param {
                turn.session.pending_start = Some(param.to_string());
            }
            return Ok(Gate::Stop(consent_screen(&content)));
        }
    }
    let param = start_param
        .map(str::to_string)
        .or_else(|| turn.session.pending_start.take());
    turn.session.pending_start = None;
    let (client, _) = services
        .clients()
        .ensure_telegram(
            &profile(turn.user),
            RegistrationSource::Bot,
            param.as_deref(),
        )
        .await?;
    if client.is_blocked() {
        turn.session = Session::default();
        return Ok(Gate::Stop(Screen::new(ui::text(
            &content,
            "common.blocked",
            &[],
        ))));
    }
    let mut conn = services.store().conn().await?;
    if client.bot_unreachable {
        conn.clients().set_bot_unreachable(client.id, false).await?;
    }
    if let Err(err) = conn
        .client_photos()
        .request_if_stale(client.id, crate::photos::REFRESH_AFTER)
        .await
    {
        tracing::debug!(error = %err, "запрос фото клиента не сохранён");
    }
    Ok(Gate::Ready(client))
}

fn document_lines(content: &BotContent, documents: &[Document], lines: &mut Lines) {
    for document in documents {
        lines.add(ui::text(
            content,
            "docs.item",
            &[
                ("title", Text(&document.title)),
                ("url", Text(&document.url)),
            ],
        ));
    }
}

fn consent_screen(content: &BotContent) -> Screen {
    let mut lines = Lines::new();
    lines.add(ui::text(content, "docs.intro", &[]));
    document_lines(content, content.consent_documents(), &mut lines);
    lines.add(ui::text(content, "docs.agree", &[]));
    Screen::new(lines.text())
        .keyboard(Keyboard::new().push(ui::button(content, "docs.btn.accept", &[], "terms:ok")))
        .section(Section::Documents)
}

pub fn documents_screen(content: &BotContent) -> Screen {
    let mut lines = Lines::new();
    lines.add(ui::text(content, "docs.list_title", &[]));
    document_lines(content, &content.documents, &mut lines);
    Screen::new(lines.text())
        .with_menu()
        .section(Section::Documents)
}

async fn accept_documents(turn: &mut Turn<'_>) -> Result<(), BotError> {
    let content = turn.services().bot_content();
    let documents = content.consent_documents();
    if let Some(version) = consent::version(documents) {
        let mut conn = turn.services().store().conn().await?;
        consent::accept(&mut conn, turn.user.id, &version, documents).await?;
    }
    Ok(())
}

async fn after_start(
    turn: &mut Turn<'_>,
    client: &Client,
    param: Option<&str>,
) -> Result<Screen, BotError> {
    turn.session.awaiting = None;
    match param {
        Some(param) if param.starts_with(login::PREFIX) => login_screen(turn, client, param).await,
        _ => main_menu(turn.ctx, client).await,
    }
}

async fn input(
    turn: &mut Turn<'_>,
    client: &Client,
    awaiting: Awaiting,
    text: &str,
) -> Result<Screen, BotError> {
    turn.session.awaiting = None;
    match awaiting {
        Awaiting::PromoCode => extras::promo_entered(turn, client, text).await,
        Awaiting::PromoForCart => buy::promo_entered(turn, client, text).await,
        Awaiting::TopUpAmount if !turn.services().payments().balance_enabled() => {
            account::balance(turn, client).await
        }
        Awaiting::TopUpAmount => account::topup_entered(turn, text).await,
        Awaiting::SupportMessage => account::support_entered(turn, client, text).await,
        Awaiting::WithdrawalAmount => extras::withdrawal_amount(turn, client, text).await,
        Awaiting::WithdrawalDetails => extras::withdrawal_details(turn, client, text).await,
    }
}

pub fn id_arg(rest: &str) -> Option<i64> {
    rest.parse::<i64>().ok().filter(|id| *id > 0)
}

async fn callback(turn: &mut Turn<'_>, client: &Client, data: &str) -> Result<Screen, BotError> {
    let (head, rest) = data.split_once(':').unwrap_or((data, ""));
    turn.session.awaiting = None;
    let screen = match head {
        "subs" => account::subscriptions(turn, client).await?,
        "sub" => match id_arg(rest) {
            Some(id) => account::subscription(turn, client, id).await?,
            None => main_menu(turn.ctx, client).await?,
        },
        "dev" => match id_arg(rest) {
            Some(id) => account::devices(turn, client, id).await?,
            None => main_menu(turn.ctx, client).await?,
        },
        "dx" => account::device_confirm(turn, rest).await?,
        "dxy" => account::device_disconnect(turn, client, rest).await?,
        "rei" => match id_arg(rest) {
            Some(id) => account::reissue_confirm(&turn.services().bot_content(), id),
            None => main_menu(turn.ctx, client).await?,
        },
        "reiy" => match id_arg(rest) {
            Some(id) => account::reissue(turn, client, id).await?,
            None => main_menu(turn.ctx, client).await?,
        },
        "bal" => account::balance(turn, client).await?,
        "top" | "topx" | "tp" if !turn.services().payments().balance_enabled() => {
            account::balance(turn, client).await?
        }
        "top" => match rest.parse::<i64>() {
            Ok(minor) => account::topup_amount(turn, minor).await?,
            Err(_) => account::balance(turn, client).await?,
        },
        "topx" => account::topup_custom(turn),
        "tp" => account::topup_start(turn, client, rest).await?,
        "sup" => match id_arg(rest) {
            Some(id) => account::support_reply_prompt(turn, client, id).await?,
            None => account::support_prompt(turn),
        },
        "trial" => account::trial(turn, client).await?,
        "buy" => buy::start(turn, client, rest).await?,
        "t" => match id_arg(rest) {
            Some(id) => buy::tariff(turn, client, TariffId(id)).await?,
            None => buy::start(turn, client, "").await?,
        },
        "p" => match id_arg(rest) {
            Some(id) => buy::period(turn, client, id).await?,
            None => buy::start(turn, client, "").await?,
        },
        "d" => match rest.parse::<u32>() {
            Ok(count) => buy::devices(turn, client, count).await?,
            Err(_) => buy::summary(turn, client).await?,
        },
        "sum" => buy::summary(turn, client).await?,
        "promo" => buy::ask_promo(turn),
        "nopromo" => {
            turn.session.promo = None;
            buy::summary(turn, client).await?
        }
        "pay" => buy::pay(turn, client, rest).await?,
        "x" => extras::entry(turn, client, rest).await?,
        "pc" => match id_arg(rest) {
            Some(id) => extras::activate_days(turn, client, id).await?,
            None => main_menu(turn.ctx, client).await?,
        },
        "ref" => extras::referral_action(turn, client, rest).await?,
        "opt" => extras::options_action(turn, client, rest).await?,
        "auth" => login_decision(turn, rest).await?,
        "docs" => documents_screen(&turn.services().bot_content()),
        _ => main_menu(turn.ctx, client).await?,
    };
    Ok(screen)
}

pub async fn tariff_name(services: &Services, tariff_id: Option<TariffId>) -> String {
    let Some(id) = tariff_id else {
        return "Подписка".into();
    };
    let found = match services.store().conn().await {
        Ok(mut conn) => conn.tariffs().get(id).await.ok(),
        Err(_) => None,
    };
    found.map_or_else(|| "Подписка".into(), |t| t.name)
}

pub fn live(views: Vec<SubscriptionView>) -> Vec<Subscription> {
    views
        .into_iter()
        .map(|v| v.subscription)
        .filter(|s| s.ended_at.is_none())
        .collect()
}

async fn trial_available(services: &Services, client: &Client) -> Result<bool, BotError> {
    if !services.settings().trial.enabled {
        return Ok(false);
    }
    let mut conn = services.store().conn().await?;
    let identities: Vec<(IdentityKind, String)> = conn
        .clients()
        .identities(client.id)
        .await?
        .into_iter()
        .map(|i| (i.kind, i.value))
        .collect();
    Ok(!conn.trials().was_claimed(&identities).await?)
}

pub fn module_buttons(services: &Services) -> Vec<&'static str> {
    let mut entries: Vec<_> = services
        .modules()
        .enabled()
        .iter()
        .flat_map(|m| m.bot_entries())
        .filter(|e| {
            matches!(
                e.key,
                promo::BOT_ENTRY | referral::BOT_ENTRY | options::BOT_ENTRY
            )
        })
        .collect();
    entries.sort_by_key(|e| e.order);
    entries.into_iter().map(|e| e.key).collect()
}

fn module_button_key(entry: &str) -> Option<&'static str> {
    match entry {
        promo::BOT_ENTRY => Some("menu.btn.promo"),
        referral::BOT_ENTRY => Some("menu.btn.referral"),
        options::BOT_ENTRY => Some("menu.btn.options"),
        _ => None,
    }
}

async fn first_name(services: &Services, client: &Client) -> String {
    let Ok(mut conn) = services.store().conn().await else {
        return String::new();
    };
    conn.clients()
        .identities(client.id)
        .await
        .ok()
        .and_then(|list| {
            list.into_iter()
                .find(|i| i.kind == IdentityKind::Telegram)
                .and_then(|i| {
                    i.meta
                        .get("first_name")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
        })
        .unwrap_or_default()
}

pub fn status_text(
    content: &BotContent,
    subscription: &Subscription,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    ui::text(content, text::status_key(subscription, now), &[])
}

async fn has_balance(services: &Services, client: &Client) -> Result<bool, BotError> {
    let mut conn = services.store().conn().await?;
    let balance = conn
        .ledger()
        .balance(client.id, Wallet::Main, services.settings().currency)
        .await?;
    Ok(balance.minor > 0)
}

pub async fn main_menu(ctx: &BotCtx, client: &Client) -> Result<Screen, BotError> {
    let services = &ctx.services;
    let content = services.bot_content();
    let c = &*content;
    let settings = services.settings();
    let now = services.now();
    let offset = settings.remna_offset();
    let subscriptions = live(services.subscriptions().list_for_client(client.id).await?);
    let wants_name = ["menu.title", "menu.greeting"]
        .iter()
        .any(|key| c.content.template(key).contains("{name}"));
    let name = if wants_name {
        first_name(services, client).await
    } else {
        String::new()
    };
    let head = [
        ("brand", Text(settings.brand_name.as_str())),
        ("name", Text(name.as_str())),
    ];
    let mut lines = Lines::new();
    lines.add(ui::text(c, "menu.title", &head));
    lines.add(ui::text(c, "menu.greeting", &head));
    if subscriptions.is_empty() {
        lines.add(ui::text(c, "menu.no_subscription", &[]));
    }
    for subscription in &subscriptions {
        let tariff = tariff_name(services, subscription.tariff_id).await;
        let status = status_text(c, subscription, now);
        lines.add(match subscription.mirror.expire_at {
            Some(at) => ui::text(
                c,
                "menu.subscription",
                &[
                    ("tariff", Text(&tariff)),
                    ("status", Html(&status)),
                    ("date", Text(&text::date(at, offset))),
                ],
            ),
            None => ui::text(
                c,
                "menu.subscription_open",
                &[("tariff", Text(&tariff)), ("status", Html(&status))],
            ),
        });
    }
    let mut buttons = Vec::new();
    if !subscriptions.is_empty() {
        buttons.extend(ui::button(c, "menu.btn.subscription", &[], "subs"));
    }
    if subscriptions.is_empty() && trial_available(services, client).await? {
        buttons.extend(ui::button(c, "menu.btn.trial", &[], "trial"));
    }
    buttons.extend(ui::button(
        c,
        if subscriptions.is_empty() {
            "menu.btn.buy"
        } else {
            "menu.btn.extend"
        },
        &[],
        "buy",
    ));
    if services.payments().balance_enabled() || has_balance(services, client).await? {
        buttons.extend(ui::button(c, "menu.btn.balance", &[], "bal"));
    }
    for entry in module_buttons(services) {
        if let Some(key) = module_button_key(entry) {
            buttons.extend(ui::button(c, key, &[], format!("x:{entry}")));
        }
    }
    if !c.documents.is_empty() {
        buttons.extend(ui::button(c, "menu.btn.documents", &[], "docs"));
    }
    buttons.extend(ui::button(c, "menu.btn.support", &[], "sup"));
    Ok(Screen::new(lines.text())
        .keyboard(Keyboard::new().grid(buttons, c.content.menu_columns()))
        .section(Section::Menu))
}

fn paysupport(ctx: &BotCtx) -> Screen {
    let content = ctx.services.bot_content();
    let settings = ctx.services.settings();
    let mut keyboard = Keyboard::new();
    if let Some(url) = settings
        .support_url
        .as_deref()
        .filter(|u| u.starts_with("http"))
    {
        keyboard = keyboard.push(ui::link(&content, "pay.btn.write_support", &[], url));
    }
    keyboard = keyboard.push(ui::button(&content, "pay.btn.support_bot", &[], "sup"));
    Screen::new(ui::text(&content, "pay.paysupport", &[]))
        .keyboard(keyboard)
        .section(Section::Support)
}

async fn login_screen(
    turn: &mut Turn<'_>,
    client: &Client,
    param: &str,
) -> Result<Screen, BotError> {
    let Some(token) = login::token_from_param(param) else {
        return main_menu(turn.ctx, client).await;
    };
    let content = turn.services().bot_content();
    let c = &*content;
    let request = match login::describe(turn.services(), token, &profile(turn.user)).await {
        Ok(request) => request,
        Err(err) => {
            return Ok(Screen::notice(ui::text(
                c,
                "login.error",
                &[("error", Text(&text::user_error(&err)))],
            ))
            .section(Section::Login));
        }
    };
    turn.session.login_token = Some(token.to_string());
    let who = match &turn.user.username {
        Some(username) => format!(
            "{} (@{}, Telegram {})",
            turn.user.first_name, username, turn.user.id
        ),
        None => format!("{} (Telegram {})", turn.user.first_name, turn.user.id),
    };
    let mut lines = Lines::new();
    lines.add(ui::text(c, "login.title", &[]));
    lines.add(ui::text(c, "login.account", &[("who", Text(&who))]));
    if let Some(ip) = &request.ip {
        lines.add(ui::text(c, "login.ip", &[("ip", Text(ip))]));
    }
    if let Some(agent) = &request.user_agent {
        lines.add(ui::text(c, "login.browser", &[("agent", Text(agent))]));
    }
    lines.add(ui::text(c, "login.hint", &[]));
    Ok(Screen::new(lines.text())
        .keyboard(
            Keyboard::new()
                .push(ui::button(c, "login.btn.confirm", &[], "auth:ok"))
                .push(ui::button(c, "login.btn.reject", &[], "auth:no")),
        )
        .section(Section::Login))
}

async fn login_decision(turn: &mut Turn<'_>, rest: &str) -> Result<Screen, BotError> {
    let content = turn.services().bot_content();
    let Some(token) = turn.session.login_token.take() else {
        return Ok(Screen::notice(ui::text(&content, "login.expired", &[])).section(Section::Login));
    };
    let result = match rest {
        "ok" => login::confirm(turn.services(), &token, &profile(turn.user))
            .await
            .map(|_| "login.confirmed"),
        _ => login::reject(turn.services(), &token)
            .await
            .map(|_| "login.rejected"),
    };
    Ok(match result {
        Ok(key) => Screen::notice(ui::text(&content, key, &[])).section(Section::Login),
        Err(err) => Screen::notice(text::user_error(&err)),
    })
}

async fn pre_checkout(ctx: &BotCtx, query: &PreCheckoutQuery) -> Result<(), BotError> {
    let request = StarsPreCheckout {
        query_id: query.id.clone(),
        invoice_payload: query.invoice_payload.clone(),
        total_amount: query.total_amount,
        currency: query.currency.clone(),
        telegram_user_id: query.from.id,
    };
    let error = match ctx.services.inbox().stars_pre_checkout(&request).await {
        Ok(()) => None,
        Err(err) => Some(text::user_error(&err)),
    };
    ctx.api
        .answer_pre_checkout(&query.id, error.as_deref())
        .await?;
    Ok(())
}

async fn stars_paid(ctx: &BotCtx, target: Target, event: &StarsPayment) -> Result<(), BotError> {
    let content = ctx.services.bot_content();
    let c = &*content;
    let deferred = Screen::notice(ui::text(c, "pay.stars_deferred", &[])).section(Section::Payment);
    let screen = match stars::settle(&ctx.services, event).await {
        Settled::Done => Screen::new(ui::text(c, "pay.stars_done", &[]))
            .keyboard(
                Keyboard::new()
                    .push(ui::button(c, "common.btn.subscription", &[], "subs"))
                    .push(ui::menu(c)),
            )
            .section(Section::Payment),
        Settled::Repeat => {
            Screen::notice(ui::text(c, "pay.stars_repeat", &[])).section(Section::Payment)
        }
        Settled::Waiting(reason) => {
            tracing::warn!(
                charge = %event.telegram_payment_charge_id,
                reason = %reason,
                "оплата звёздами сохранена и будет проведена повторно"
            );
            deferred
        }
        Settled::Failed(reason) => {
            tracing::error!(
                charge = %event.telegram_payment_charge_id,
                reason = %reason,
                "оплата звёздами не проведена"
            );
            deferred
        }
    };
    ctx.show(target, &screen).await
}

async fn stars_refunded(ctx: &BotCtx, event: &StarsPayment) {
    match stars::settle(&ctx.services, event).await {
        Settled::Done | Settled::Repeat => {}
        Settled::Waiting(reason) => tracing::info!(
            charge = %event.telegram_payment_charge_id,
            reason = %reason,
            "возврат звёзд сохранён и будет записан позже"
        ),
        Settled::Failed(reason) => tracing::warn!(
            charge = %event.telegram_payment_charge_id,
            reason = %reason,
            "возврат звёзд не записан"
        ),
    }
}
