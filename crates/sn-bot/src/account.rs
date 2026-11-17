use crate::BotError;
use crate::handler::{Screen, Turn, live, status_text, tariff_name};
use crate::session::{Awaiting, DevicePick};
use crate::telegram::Keyboard;
use crate::text;
use crate::ui::{self, Html, Lines, Text};
use sn_domain::bot_content::Section;
use sn_domain::client::Client;
use sn_domain::format;
use sn_domain::ids::{SubscriptionId, TicketId};
use sn_domain::ledger::Wallet;
use sn_domain::money::{Currency, Money, PayCurrency, parse_units};
use sn_domain::pricing::topup_quote;
use sn_domain::traffic::{NextReset, next_reset};
use sn_payments::{MethodsContext, ProviderKind};
use sn_services::bot_content::BotContent;
use sn_services::checkout::{Channel, CheckoutResult};
use sn_services::support::Delivered;
use sn_services::{Actor, ServiceError};
use sn_store::{Db, StoreError};

pub fn payment_screen(c: &BotContent, result: &CheckoutResult, what: &str) -> Screen {
    match result {
        CheckoutResult::Redirect { pay_url, .. } if pay_url.starts_with("http") => {
            Screen::new(ui::text(c, "pay.ready", &[("what", Html(what))]))
                .keyboard(
                    Keyboard::new()
                        .push(ui::link(c, "pay.btn.pay", &[], pay_url.clone()))
                        .push(ui::menu(c)),
                )
                .section(Section::Payment)
        }
        CheckoutResult::Redirect { pay_url, .. } => Screen::notice(ui::text(
            c,
            "pay.link",
            &[("what", Html(what)), ("url", Text(pay_url))],
        ))
        .section(Section::Payment),
        CheckoutResult::Completed { .. } => Screen::new(ui::text(c, "pay.balance_done", &[]))
            .keyboard(
                Keyboard::new()
                    .push(ui::button(c, "common.btn.subscription", &[], "subs"))
                    .push(ui::menu(c)),
            )
            .section(Section::Payment),
    }
}

pub async fn subscriptions(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content();
    let c = &*content;
    let list = live(services.subscriptions().list_for_client(client.id).await?);
    match list.as_slice() {
        [] => Ok(Screen::new(ui::text(c, "sub.none", &[]))
            .keyboard(
                Keyboard::new()
                    .push(ui::button(c, "sub.btn.buy", &[], "buy"))
                    .push(ui::menu(c)),
            )
            .section(Section::Subscription)),
        [only] => subscription(turn, client, only.id.0).await,
        many => {
            let offset = services.settings().remna_offset();
            let mut keyboard = Keyboard::new();
            for sub in many {
                let name = tariff_name(&services, sub.tariff_id).await;
                let button = match sub.mirror.expire_at {
                    Some(at) => ui::button(
                        c,
                        "sub.btn.item",
                        &[
                            ("tariff", Text(&name)),
                            ("date", Text(&text::day_month(at, offset))),
                        ],
                        format!("sub:{}", sub.id),
                    ),
                    None => ui::button(
                        c,
                        "sub.btn.item_open",
                        &[("tariff", Text(&name))],
                        format!("sub:{}", sub.id),
                    ),
                };
                keyboard = keyboard.push(button);
            }
            Ok(Screen::new(ui::text(c, "sub.choose", &[]))
                .keyboard(keyboard.push(ui::menu(c)))
                .section(Section::Subscription))
        }
    }
}

fn not_found(err: &ServiceError) -> bool {
    matches!(
        err,
        ServiceError::NotFound(_) | ServiceError::Store(StoreError::NotFound(_))
    )
}

pub async fn subscription(
    turn: &mut Turn<'_>,
    client: &Client,
    id: i64,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content();
    let c = &*content;
    let actor = Actor::Client(client.id);
    let sid = SubscriptionId(id);
    let (view, link_error) = match services.subscriptions().view(sid, actor, true).await {
        Ok(view) => (view, None),
        Err(err) if not_found(&err) => {
            return Ok(
                Screen::notice(ui::text(c, "sub.not_found", &[])).section(Section::Subscription)
            );
        }
        Err(err) => match services.subscriptions().view(sid, actor, false).await {
            Ok(view) => (view, Some(err)),
            Err(_) => return Ok(Screen::notice(text::user_error(&err))),
        },
    };
    let sub = &view.subscription;
    let now = services.now();
    let settings = services.settings();
    let offset = settings.remna_offset();
    let name = tariff_name(&services, sub.tariff_id).await;
    let status = status_text(c, sub, now);
    let mut lines = Lines::new();
    lines.add(ui::text(
        c,
        "sub.title",
        &[("tariff", Text(&name)), ("status", Html(&status))],
    ));
    match sub.mirror.expire_at {
        Some(at) if at > now => lines.add(ui::text(
            c,
            "sub.until",
            &[
                ("date", Text(&text::date(at, offset))),
                ("days", Text(&text::plural_days(text::days_left(at, now)))),
            ],
        )),
        Some(at) => lines.add(ui::text(
            c,
            "sub.ended",
            &[("date", Text(&text::date(at, offset)))],
        )),
        None => {}
    }
    match sub.mirror.traffic_limit_bytes.filter(|l| *l > 0) {
        Some(limit) => {
            let used = sub.mirror.traffic_used_bytes.unwrap_or(0);
            let reset = match sub.tariff_id {
                Some(tariff_id) => {
                    let mut conn = services.store().conn().await?;
                    conn.tariffs()
                        .get(tariff_id)
                        .await
                        .ok()
                        .map(|t| next_reset(t.traffic.reset, sub.created_at, now, offset))
                }
                None => None,
            };
            let reset = match reset {
                Some(NextReset::At(at)) => ui::text(
                    c,
                    "sub.traffic_reset_date",
                    &[("date", Text(&text::day_month(at, offset)))],
                ),
                Some(NextReset::OnPayment) => ui::text(c, "sub.traffic_reset_payment", &[]),
                _ => String::new(),
            };
            lines.add(ui::text(
                c,
                "sub.traffic",
                &[
                    ("used", Text(&format::traffic(used))),
                    ("limit", Text(&format::traffic(limit))),
                    ("reset", Html(&reset)),
                ],
            ));
        }
        None => lines.add(ui::text(c, "sub.traffic_unlimited", &[])),
    }
    match (sub.mirror.devices_count, sub.mirror.hwid_limit) {
        (Some(count), Some(limit)) => lines.add(ui::text(
            c,
            "sub.devices",
            &[
                ("count", Text(&count.to_string())),
                ("limit", Text(&limit.to_string())),
            ],
        )),
        (None, Some(limit)) => lines.add(ui::text(
            c,
            "sub.devices_limit",
            &[("limit", Text(&limit.to_string()))],
        )),
        _ => {}
    }
    lines.add(ui::text(
        c,
        if sub.autorenew.enabled {
            "sub.autorenew_on"
        } else {
            "sub.autorenew_off"
        },
        &[],
    ));
    let mut keyboard = Keyboard::new();
    match (&view.subscription_url, &link_error) {
        (Some(url), _) => {
            lines.add(ui::text(c, "sub.link", &[("url", Text(url))]));
            if url.starts_with("http") {
                keyboard = keyboard.push(ui::link(c, "sub.btn.connect", &[], url.clone()));
            }
        }
        (None, Some(err)) => {
            lines.add(ui::text(
                c,
                "sub.link_error",
                &[("error", Text(&text::user_error(err)))],
            ));
        }
        (None, None) => {}
    }
    if sub.remna.is_some() {
        keyboard = keyboard
            .push(ui::button(c, "sub.btn.devices", &[], format!("dev:{id}")))
            .push(ui::button(c, "sub.btn.reissue", &[], format!("rei:{id}")));
    }
    keyboard = keyboard
        .push(ui::button(c, "sub.btn.extend", &[], format!("buy:{id}")))
        .pair(ui::button(c, "sub.btn.all", &[], "subs"), ui::menu(c));
    Ok(Screen::new(lines.text())
        .keyboard(keyboard)
        .section(Section::Subscription))
}

pub async fn devices(turn: &mut Turn<'_>, client: &Client, id: i64) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content();
    let c = &*content;
    let list = match services
        .subscriptions()
        .devices(SubscriptionId(id), Actor::Client(client.id))
        .await
    {
        Ok(list) => list,
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    let back = Keyboard::new().pair(ui::back(c, format!("sub:{id}")), ui::menu(c));
    if list.is_empty() {
        turn.session.devices = None;
        return Ok(Screen::new(ui::text(c, "dev.empty", &[]))
            .keyboard(back)
            .section(Section::Devices));
    }
    let offset = services.settings().remna_offset();
    let mut lines = Lines::new();
    lines.add(ui::text(c, "dev.title", &[]));
    let mut keyboard = Keyboard::new();
    for (index, device) in list.iter().enumerate() {
        let details: Vec<String> = [
            match (&device.platform, &device.os_version) {
                (Some(platform), Some(version)) => Some(format!("{platform} {version}")),
                (Some(platform), None) => Some(platform.clone()),
                _ => None,
            },
            device.app.clone(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let number = (index + 1).to_string();
        let date = text::day_month(device.added_at, offset);
        lines.add(if details.is_empty() {
            ui::text(
                c,
                "dev.line_plain",
                &[
                    ("n", Text(&number)),
                    ("title", Text(&device.title)),
                    ("date", Text(&date)),
                ],
            )
        } else {
            ui::text(
                c,
                "dev.line",
                &[
                    ("n", Text(&number)),
                    ("title", Text(&device.title)),
                    ("details", Text(&details.join(" · "))),
                    ("date", Text(&date)),
                ],
            )
        });
        keyboard = keyboard.push(ui::button(
            c,
            "dev.btn.disconnect",
            &[("title", Text(&device.title))],
            format!("dx:{index}"),
        ));
    }
    turn.session.devices = Some(DevicePick {
        subscription_id: SubscriptionId(id),
        hwids: list.into_iter().map(|d| (d.hwid, d.title)).collect(),
    });
    keyboard.inline_keyboard.extend(back.inline_keyboard);
    Ok(Screen::new(lines.text())
        .keyboard(keyboard)
        .section(Section::Devices))
}

fn picked(turn: &Turn<'_>, rest: &str) -> Option<(SubscriptionId, usize, String, String)> {
    let index: usize = rest.parse().ok()?;
    let pick = turn.session.devices.as_ref()?;
    let (hwid, title) = pick.hwids.get(index)?;
    Some((pick.subscription_id, index, hwid.clone(), title.clone()))
}

pub async fn device_confirm(turn: &mut Turn<'_>, rest: &str) -> Result<Screen, BotError> {
    let content = turn.services().bot_content();
    let c = &*content;
    let Some((sub, index, _, title)) = picked(turn, rest) else {
        return Ok(Screen::notice(ui::text(c, "dev.stale", &[])).section(Section::Devices));
    };
    Ok(
        Screen::new(ui::text(c, "dev.confirm", &[("title", Text(&title))]))
            .keyboard(
                Keyboard::new()
                    .push(ui::button(
                        c,
                        "dev.btn.disconnect_yes",
                        &[],
                        format!("dxy:{index}"),
                    ))
                    .push(ui::cancel(c, format!("dev:{sub}"))),
            )
            .section(Section::Devices),
    )
}

pub async fn device_disconnect(
    turn: &mut Turn<'_>,
    client: &Client,
    rest: &str,
) -> Result<Screen, BotError> {
    let content = turn.services().bot_content();
    let c = &*content;
    let Some((sub, _, hwid, title)) = picked(turn, rest) else {
        return Ok(Screen::notice(ui::text(c, "dev.stale", &[])).section(Section::Devices));
    };
    match turn
        .services()
        .subscriptions()
        .disconnect_device(sub, &hwid, Actor::Client(client.id))
        .await
    {
        Ok(()) => {
            turn.session.devices = None;
            Ok(
                Screen::new(ui::text(c, "dev.disconnected", &[("title", Text(&title))]))
                    .keyboard(
                        Keyboard::new()
                            .push(ui::button(c, "sub.btn.devices", &[], format!("dev:{sub}")))
                            .push(ui::menu(c)),
                    )
                    .section(Section::Devices),
            )
        }
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}

pub fn reissue_confirm(c: &BotContent, id: i64) -> Screen {
    Screen::new(ui::text(c, "sub.reissue_confirm", &[]))
        .keyboard(
            Keyboard::new()
                .push(ui::button(
                    c,
                    "sub.btn.reissue_yes",
                    &[],
                    format!("reiy:{id}"),
                ))
                .push(ui::cancel(c, format!("sub:{id}"))),
        )
        .section(Section::Subscription)
}

pub async fn reissue(turn: &mut Turn<'_>, client: &Client, id: i64) -> Result<Screen, BotError> {
    let content = turn.services().bot_content();
    let c = &*content;
    match turn
        .services()
        .subscriptions()
        .reissue_link(SubscriptionId(id), Actor::Client(client.id))
        .await
    {
        Ok(url) => {
            let mut keyboard = Keyboard::new();
            if url.starts_with("http") {
                keyboard = keyboard.push(ui::link(c, "sub.btn.connect", &[], url.clone()));
            }
            Ok(
                Screen::new(ui::text(c, "sub.reissued", &[("url", Text(&url))]))
                    .keyboard(keyboard.push(ui::menu(c)))
                    .section(Section::Subscription),
            )
        }
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}

fn presets(currency: Currency) -> [i64; 4] {
    match currency {
        Currency::Usd => [500, 1_000, 2_000, 5_000],
        Currency::Rub => [30_000, 50_000, 100_000, 200_000],
    }
}

pub async fn balance(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content();
    let c = &*content;
    let settings = services.settings();
    let currency = settings.currency;
    let (main, referral) = {
        let mut conn = services.store().conn().await?;
        (
            conn.ledger()
                .balance(client.id, Wallet::Main, currency)
                .await?,
            conn.ledger()
                .balance(client.id, Wallet::Referral, currency)
                .await?,
        )
    };
    let mut lines = Lines::new();
    lines.add(ui::text(
        c,
        "bal.amount",
        &[("amount", Text(&format::money(main)))],
    ));
    if referral.minor > 0 {
        lines.add(ui::text(
            c,
            "bal.referral",
            &[("amount", Text(&format::money(referral)))],
        ));
    }
    if !services.payments().balance_enabled() {
        lines.add(ui::text(c, "bal.off_note", &[]));
        return Ok(Screen::new(lines.text())
            .keyboard(Keyboard::new().push(ui::menu(c)))
            .section(Section::Balance));
    }
    lines.add(ui::text(c, "bal.note", &[]));
    let limits = settings.topup_limits();
    let buttons: Vec<_> = presets(currency)
        .into_iter()
        .filter(|m| *m >= limits.min.minor && *m <= limits.max.minor)
        .filter_map(|minor| {
            ui::button(
                c,
                "bal.btn.preset",
                &[("amount", Text(&format::money(Money::new(minor, currency))))],
                format!("top:{minor}"),
            )
        })
        .collect();
    Ok(Screen::new(lines.text())
        .keyboard(
            Keyboard::new()
                .grid(buttons, 2)
                .push(ui::button(c, "bal.btn.custom", &[], "topx"))
                .push(ui::menu(c)),
        )
        .section(Section::Balance))
}

pub async fn topup_amount(turn: &mut Turn<'_>, minor: i64) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content();
    let c = &*content;
    let settings = services.settings();
    let amount = Money::new(minor, settings.currency);
    if let Err(err) = topup_quote(amount, settings.topup_limits()) {
        turn.session.wait_for(Awaiting::TopUpAmount);
        return Ok(Screen::notice(ui::text(
            c,
            "bal.bad_amount",
            &[("error", Text(&text::capitalize(&err.to_string())))],
        ))
        .section(Section::Balance));
    }
    let methods: Vec<_> = services
        .payments()
        .checkout_methods(&MethodsContext {
            currency: settings.currency,
            total: amount,
            stars_total: None,
            balance: None,
            channel: Some(sn_domain::access::ClientChannel::Bot),
        })
        .into_iter()
        .filter(|m| m.kind == ProviderKind::External && m.unavailable.is_none())
        .collect();
    if methods.is_empty() {
        return Ok(Screen::notice(ui::text(c, "bal.no_methods", &[])).section(Section::Balance));
    }
    turn.session.topup_minor = Some(minor);
    let mut keyboard = Keyboard::new();
    for method in methods {
        let data = format!("tp:{}", method.provider);
        keyboard = keyboard.push(if method.hint.is_empty() {
            ui::button(c, "bal.btn.method", &[("method", Text(method.title))], data)
        } else {
            ui::button(
                c,
                "bal.btn.method_hint",
                &[("method", Text(method.title)), ("hint", Text(method.hint))],
                data,
            )
        });
    }
    Ok(Screen::new(ui::text(
        c,
        "bal.choose_method",
        &[("amount", Text(&format::money(amount)))],
    ))
    .keyboard(keyboard.push(ui::back(c, "bal")))
    .section(Section::Balance))
}

pub fn topup_custom(turn: &mut Turn<'_>) -> Screen {
    turn.session.wait_for(Awaiting::TopUpAmount);
    let content = turn.services().bot_content();
    let example = match turn.services().settings().currency {
        Currency::Usd => "например 10 или 12.50",
        Currency::Rub => "например 500",
    };
    Screen::new(ui::text(
        &content,
        "bal.enter_amount",
        &[("example", Text(example))],
    ))
    .keyboard(Keyboard::new().push(ui::cancel(&content, "bal")))
    .section(Section::Balance)
}

pub async fn topup_entered(turn: &mut Turn<'_>, raw: &str) -> Result<Screen, BotError> {
    let currency = turn.services().settings().currency;
    let cleaned: String = raw
        .trim()
        .trim_end_matches(['$', '₽'])
        .trim_start_matches('$')
        .replace(',', ".")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{a0}')
        .collect();
    match parse_units(&cleaned, PayCurrency::from(currency)) {
        Some(minor) if minor > 0 => topup_amount(turn, minor).await,
        _ => {
            turn.session.wait_for(Awaiting::TopUpAmount);
            let content = turn.services().bot_content();
            Ok(Screen::new(ui::text(&content, "bal.not_understood", &[]))
                .keyboard(Keyboard::new().push(ui::cancel(&content, "bal")))
                .section(Section::Balance))
        }
    }
}

pub async fn topup_start(
    turn: &mut Turn<'_>,
    client: &Client,
    provider: &str,
) -> Result<Screen, BotError> {
    let Some(minor) = turn.session.topup_minor else {
        return balance(turn, client).await;
    };
    let services = turn.services().clone();
    let content = services.bot_content();
    let amount = Money::new(minor, services.settings().currency);
    match services
        .checkout()
        .topup(client.id, amount, provider, Channel::Bot)
        .await
    {
        Ok(result) => {
            turn.session.topup_minor = None;
            let what = ui::text(
                &content,
                "pay.what_topup",
                &[("amount", Text(&format::money(amount)))],
            );
            Ok(payment_screen(&content, &result, &what))
        }
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}

pub async fn support_reply_prompt(
    turn: &mut Turn<'_>,
    client: &Client,
    ticket: i64,
) -> Result<Screen, BotError> {
    let open = match turn
        .services()
        .support()
        .telegram_ticket(client.id, Some(TicketId(ticket)))
        .await
    {
        Ok(open) => open,
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    let Some(open) = open else {
        return Ok(support_prompt(turn));
    };
    turn.session.support_ticket = Some(open.0);
    turn.session.wait_for(Awaiting::SupportMessage);
    let content = turn.services().bot_content();
    let c = &*content;
    Ok(Screen::new(ui::text(
        c,
        "sup.reply_prompt",
        &[("id", Text(&open.to_string()))],
    ))
    .keyboard(Keyboard::new().push(ui::cancel(c, "m")))
    .section(Section::Support))
}

pub fn support_prompt(turn: &mut Turn<'_>) -> Screen {
    turn.session.support_ticket = None;
    turn.session.wait_for(Awaiting::SupportMessage);
    let content = turn.services().bot_content();
    let c = &*content;
    let mut keyboard = Keyboard::new();
    if let Some(url) = turn
        .services()
        .settings()
        .support_url
        .as_deref()
        .filter(|u| u.starts_with("http"))
    {
        keyboard = keyboard.push(ui::link(c, "sup.btn.other", &[], url));
    }
    Screen::new(ui::text(c, "sup.prompt", &[]))
        .keyboard(keyboard.push(ui::cancel(c, "m")))
        .section(Section::Support)
}

pub async fn support_entered(
    turn: &mut Turn<'_>,
    client: &Client,
    body: &str,
) -> Result<Screen, BotError> {
    let preferred = turn.session.support_ticket.take();
    if body.trim().is_empty() {
        return match preferred {
            Some(ticket) => support_reply_prompt(turn, client, ticket).await,
            None => Ok(support_prompt(turn)),
        };
    }
    deliver(turn, client, body, preferred.map(TicketId)).await
}

pub async fn support_followup(
    turn: &mut Turn<'_>,
    client: &Client,
    body: &str,
) -> Result<Option<Screen>, BotError> {
    if body.trim().is_empty() {
        return Ok(None);
    }
    match turn
        .services()
        .support()
        .telegram_ticket(client.id, None)
        .await
    {
        Ok(Some(ticket)) => Ok(Some(deliver(turn, client, body, Some(ticket)).await?)),
        Ok(None) => Ok(None),
        Err(err) => Ok(Some(Screen::notice(text::user_error(&err)))),
    }
}

async fn deliver(
    turn: &mut Turn<'_>,
    client: &Client,
    body: &str,
    preferred: Option<TicketId>,
) -> Result<Screen, BotError> {
    let content = turn.services().bot_content();
    let (key, id) = match turn
        .services()
        .support()
        .from_telegram(client.id, body, preferred)
        .await
    {
        Ok(Delivered::Opened(id)) => ("sup.sent", id),
        Ok(Delivered::Added(id)) => ("sup.added", id),
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    Ok(
        Screen::notice(ui::text(&content, key, &[("id", Text(&id.to_string()))]))
            .section(Section::Support),
    )
}

pub async fn trial(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let content = turn.services().bot_content();
    let c = &*content;
    match turn.services().checkout().start_trial(client.id).await {
        Ok(_) => Ok(Screen::new(ui::text(c, "sub.trial_started", &[]))
            .keyboard(
                Keyboard::new()
                    .push(ui::button(c, "common.btn.subscription", &[], "subs"))
                    .push(ui::menu(c)),
            )
            .section(Section::Subscription)),
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}
