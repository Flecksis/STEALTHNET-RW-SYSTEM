use crate::BotError;
use crate::handler::{Screen, Turn, id_arg, live, main_menu, tariff_name};
use crate::session::{Awaiting, WithdrawalDraft};
use crate::telegram::{Button, Keyboard};
use crate::text;
use crate::ui::{self, Html, Lines, Text};
use sn_domain::bot_content::Section;
use sn_domain::client::Client;
use sn_domain::format;
use sn_domain::ids::SubscriptionId;
use sn_domain::module::ModuleId;
use sn_modules::ModuleError;
use sn_modules::catalog::options::{self, OptionsError};
use sn_modules::catalog::promo::{self, PromoError, PromoRejection, Purpose};
use sn_modules::catalog::referral::{
    self, PayoutMethod, ReferralError, WithdrawalRequest, WithdrawalStatus,
};
use sn_services::ServiceError;

fn module_error(err: ModuleError) -> String {
    text::user_error(&ServiceError::Module(err))
}

pub async fn entry(turn: &mut Turn<'_>, client: &Client, key: &str) -> Result<Screen, BotError> {
    let modules = turn.services().modules().clone();
    match key {
        promo::BOT_ENTRY if modules.is_enabled(ModuleId::Promo) => Ok(promo_prompt(turn)),
        referral::BOT_ENTRY if modules.is_enabled(ModuleId::Referral) => {
            referral_screen(turn, client).await
        }
        options::BOT_ENTRY if modules.is_enabled(ModuleId::Options) => {
            options_start(turn, client).await
        }
        _ => main_menu(turn.ctx, client).await,
    }
}

fn promo_prompt(turn: &mut Turn<'_>) -> Screen {
    turn.session.wait_for(Awaiting::PromoCode);
    let content = turn.services().bot_content();
    Screen::new(ui::text(&content, "promo.prompt", &[]))
        .keyboard(Keyboard::new().push(ui::cancel(&content, "m")))
        .section(Section::Promo)
}

pub async fn promo_entered(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content();
    let c = &*content;
    let ctx = services.module_context();
    let checked = promo::check(
        &ctx,
        client.id,
        raw,
        Purpose::Check {
            tariff: None,
            currency: services.settings().currency,
        },
    )
    .await;
    let code = match checked {
        Ok(code) => code,
        Err(PromoError::Rejected(rejection)) => {
            return Ok(Screen::new(rejection.message())
                .keyboard(
                    Keyboard::new()
                        .push(ui::button(
                            c,
                            "buy.btn.promo_other",
                            &[],
                            format!("x:{}", promo::BOT_ENTRY),
                        ))
                        .push(ui::menu(c)),
                )
                .section(Section::Promo));
        }
        Err(PromoError::Module(err)) => return Ok(Screen::notice(module_error(err))),
    };
    if code.is_discount() {
        let label = code.label();
        turn.session.promo = Some(code.code);
        return Ok(
            Screen::new(ui::text(c, "promo.saved", &[("code", Text(&label))]))
                .keyboard(
                    Keyboard::new()
                        .push(ui::button(c, "promo.btn.choose_tariff", &[], "buy"))
                        .push(ui::menu(c)),
                )
                .section(Section::Promo),
        );
    }
    let subscriptions: Vec<_> = live(services.subscriptions().list_for_client(client.id).await?)
        .into_iter()
        .filter(|s| s.remna.is_some())
        .collect();
    match subscriptions.as_slice() {
        [] => Ok(Screen::notice(PromoRejection::NoSubscription.message()).section(Section::Promo)),
        [only] => {
            turn.session.days_code = Some(code.code);
            activate_days(turn, client, only.id.0).await
        }
        many => {
            turn.session.days_code = Some(code.code);
            let mut keyboard = Keyboard::new();
            for sub in many {
                let name = tariff_name(&services, sub.tariff_id).await;
                keyboard = keyboard.push(ui::button(
                    c,
                    "sub.btn.item_open",
                    &[("tariff", Text(&name))],
                    format!("pc:{}", sub.id),
                ));
            }
            Ok(Screen::new(ui::text(c, "promo.which_sub", &[]))
                .keyboard(keyboard.push(ui::cancel(c, "m")))
                .section(Section::Promo))
        }
    }
}

pub async fn activate_days(
    turn: &mut Turn<'_>,
    client: &Client,
    subscription_id: i64,
) -> Result<Screen, BotError> {
    let Some(code) = turn.session.days_code.take() else {
        return Ok(promo_prompt(turn));
    };
    let content = turn.services().bot_content();
    let c = &*content;
    let ctx = turn.services().module_context();
    match promo::activate(&ctx, client.id, &code, SubscriptionId(subscription_id)).await {
        Ok(done) => Ok(Screen::new(ui::text(
            c,
            "promo.days_added",
            &[("days", Text(&format::days(done.days)))],
        ))
        .keyboard(
            Keyboard::new()
                .push(ui::button(
                    c,
                    "common.btn.subscription",
                    &[],
                    format!("sub:{subscription_id}"),
                ))
                .push(ui::menu(c)),
        )
        .section(Section::Promo)),
        Err(PromoError::Rejected(rejection)) => {
            Ok(Screen::notice(rejection.message()).section(Section::Promo))
        }
        Err(PromoError::Module(err)) => Ok(Screen::notice(module_error(err))),
    }
}

fn back_to_referral(turn: &Turn<'_>, key: &str) -> Keyboard {
    let content = turn.services().bot_content();
    Keyboard::new().push(ui::button(
        &content,
        key,
        &[],
        format!("x:{}", referral::BOT_ENTRY),
    ))
}

async fn referral_screen(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let content = turn.services().bot_content();
    let c = &*content;
    let ctx = turn.services().module_context();
    let overview = referral::overview(&ctx, client.id).await?;
    let link = overview.bot_link.clone().or_else(|| {
        turn.ctx
            .username
            .as_deref()
            .map(|username| referral::invite_link(username, &overview.code))
    });
    let mut lines = Lines::new();
    lines.add(ui::text(c, "ref.title", &[]));
    lines.add(ui::text(
        c,
        "ref.percent",
        &[("percent", Text(&overview.levels[0].to_string()))],
    ));
    if let Some(link) = &link {
        lines.add(ui::text(c, "ref.link", &[("link", Text(link))]));
    }
    lines.add(ui::text(
        c,
        "ref.invited",
        &[("count", Text(&overview.invited.to_string()))],
    ));
    lines.add(ui::text(
        c,
        "ref.earned",
        &[("amount", Text(&format::money(overview.earned)))],
    ));
    lines.add(ui::text(
        c,
        "ref.balance",
        &[("amount", Text(&format::money(overview.balance)))],
    ));
    let mut keyboard = Keyboard::new();
    let pending: Vec<_> = overview
        .withdrawals
        .iter()
        .filter(|w| {
            matches!(
                w.status,
                WithdrawalStatus::Pending | WithdrawalStatus::Holding
            )
        })
        .collect();
    if !pending.is_empty() {
        lines.blank();
        for withdrawal in &pending {
            let id = withdrawal.id.to_string();
            let status = withdrawal.status.title_ru().to_lowercase();
            lines.add(ui::text(
                c,
                "ref.withdrawal",
                &[
                    ("id", Text(&id)),
                    ("amount", Text(&format::money(withdrawal.amount))),
                    ("status", Text(&status)),
                ],
            ));
            if withdrawal.status == WithdrawalStatus::Pending {
                keyboard = keyboard.push(ui::button(
                    c,
                    "ref.btn.cancel_withdrawal",
                    &[("id", Text(&id))],
                    format!("ref:c:{}", withdrawal.id),
                ));
            }
        }
    }
    if overview.withdrawals_enabled && overview.balance.minor > 0 {
        keyboard = keyboard.push(ui::button(c, "ref.btn.withdraw", &[], "ref:w"));
    }
    if let Some(link) = link {
        keyboard = keyboard.push(ui::link(
            c,
            "ref.btn.share",
            &[],
            format!("https://t.me/share/url?url={}", text::url_encode(&link)),
        ));
    }
    Ok(Screen::new(lines.text())
        .keyboard(keyboard.push(ui::menu(c)))
        .section(Section::Referral))
}

pub async fn referral_action(
    turn: &mut Turn<'_>,
    client: &Client,
    rest: &str,
) -> Result<Screen, BotError> {
    if !turn.services().modules().is_enabled(ModuleId::Referral) {
        return main_menu(turn.ctx, client).await;
    }
    let content = turn.services().bot_content();
    let c = &*content;
    let (action, arg) = rest.split_once(':').unwrap_or((rest, ""));
    match action {
        "w" => {
            let mut keyboard = Keyboard::new();
            for method in PayoutMethod::ALL {
                keyboard = keyboard.row(vec![Button::callback(
                    method.title_ru(),
                    format!("ref:m:{}", method.as_str()),
                )]);
            }
            Ok(Screen::new(ui::text(c, "ref.how", &[]))
                .keyboard(keyboard.push(ui::cancel(c, format!("x:{}", referral::BOT_ENTRY))))
                .section(Section::Referral))
        }
        "m" => {
            let Some(method) = PayoutMethod::parse(arg) else {
                return referral_screen(turn, client).await;
            };
            turn.session.withdrawal = Some(WithdrawalDraft {
                method: Some(method),
                amount: None,
            });
            turn.session.wait_for(Awaiting::WithdrawalAmount);
            let ctx = turn.services().module_context();
            let overview = referral::overview(&ctx, client.id).await?;
            let minimum = overview
                .withdraw_min
                .map(|m| ui::text(c, "ref.minimum", &[("amount", Text(&format::money(m)))]))
                .unwrap_or_default();
            Ok(Screen::new(ui::text(
                c,
                "ref.amount",
                &[
                    ("amount", Text(&format::money(overview.balance))),
                    ("minimum", Html(&minimum)),
                ],
            ))
            .keyboard(Keyboard::new().push(ui::cancel(c, format!("x:{}", referral::BOT_ENTRY))))
            .section(Section::Referral))
        }
        "c" => {
            let Some(id) = id_arg(arg) else {
                return referral_screen(turn, client).await;
            };
            let ctx = turn.services().module_context();
            match referral::cancel_withdrawal(&ctx, client.id, id).await {
                Ok(withdrawal) => Ok(Screen::new(ui::text(
                    c,
                    "ref.cancelled",
                    &[
                        ("id", Text(&withdrawal.id.to_string())),
                        ("amount", Text(&format::money(withdrawal.amount))),
                    ],
                ))
                .keyboard(back_to_referral(turn, "common.btn.back"))
                .section(Section::Referral)),
                Err(err) => Ok(Screen::notice(referral_error(err))),
            }
        }
        _ => referral_screen(turn, client).await,
    }
}

fn referral_error(err: ReferralError) -> String {
    match err {
        ReferralError::Rejected(rejection) => rejection.message(),
        ReferralError::Module(err) => module_error(err),
    }
}

pub async fn withdrawal_amount(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    let Some(method) = turn.session.withdrawal.as_ref().and_then(|w| w.method) else {
        return referral_screen(turn, client).await;
    };
    let amount = raw
        .trim()
        .trim_start_matches('$')
        .trim_end_matches(['$', '₽'])
        .replace(',', ".")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{a0}')
        .collect::<String>();
    turn.session.withdrawal = Some(WithdrawalDraft {
        method: Some(method),
        amount: Some(amount),
    });
    turn.session.wait_for(Awaiting::WithdrawalDetails);
    let content = turn.services().bot_content();
    Ok(Screen::new(ui::text(
        &content,
        "ref.details",
        &[("hint", Text(method.details_hint()))],
    ))
    .keyboard(back_to_referral(turn, "common.btn.cancel"))
    .section(Section::Referral))
}

pub async fn withdrawal_details(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    let Some(WithdrawalDraft {
        method: Some(method),
        amount: Some(amount),
    }) = turn.session.withdrawal.take()
    else {
        return referral_screen(turn, client).await;
    };
    let content = turn.services().bot_content();
    let c = &*content;
    let ctx = turn.services().module_context();
    match referral::request_withdrawal(
        &ctx,
        client.id,
        &WithdrawalRequest {
            amount,
            method,
            details: raw.to_string(),
        },
    )
    .await
    {
        Ok(withdrawal) => Ok(Screen::new(ui::text(
            c,
            "ref.requested",
            &[
                ("id", Text(&withdrawal.id.to_string())),
                ("amount", Text(&format::money(withdrawal.amount))),
            ],
        ))
        .keyboard(back_to_referral(turn, "common.btn.back"))
        .section(Section::Referral)),
        Err(err) => Ok(Screen::new(referral_error(err))
            .keyboard(
                Keyboard::new()
                    .push(ui::button(c, "ref.btn.retry", &[], "ref:w"))
                    .push(ui::menu(c)),
            )
            .section(Section::Referral)),
    }
}

async fn options_start(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let content = services.bot_content();
    let c = &*content;
    let subscriptions: Vec<_> = live(services.subscriptions().list_for_client(client.id).await?)
        .into_iter()
        .filter(|s| s.remna.is_some())
        .collect();
    match subscriptions.as_slice() {
        [] => Ok(Screen::notice(ui::text(c, "opt.no_subscription", &[])).section(Section::Options)),
        [only] => options_offer(turn, client, only.id).await,
        many => {
            let mut keyboard = Keyboard::new();
            for sub in many {
                let name = tariff_name(&services, sub.tariff_id).await;
                keyboard = keyboard.push(ui::button(
                    c,
                    "sub.btn.item_open",
                    &[("tariff", Text(&name))],
                    format!("opt:{}", sub.id),
                ));
            }
            Ok(Screen::new(ui::text(c, "opt.which_sub", &[]))
                .keyboard(keyboard.push(ui::menu(c)))
                .section(Section::Options))
        }
    }
}

async fn offer(
    turn: &Turn<'_>,
    client: &Client,
    subscription_id: SubscriptionId,
) -> Result<Result<options::DevicesOffer, String>, BotError> {
    let services = turn.services();
    let mut conn = services.store().conn().await?;
    let result = options::offer(
        &mut conn,
        client.id,
        subscription_id,
        services.settings().currency,
        services.now(),
    )
    .await;
    Ok(match result {
        Ok(offer) => Ok(offer),
        Err(OptionsError::Rejected(rejection)) => Err(rejection.message()),
        Err(OptionsError::Module(err)) => Err(module_error(err)),
    })
}

async fn options_offer(
    turn: &mut Turn<'_>,
    client: &Client,
    subscription_id: SubscriptionId,
) -> Result<Screen, BotError> {
    let content = turn.services().bot_content();
    let c = &*content;
    let offer = match offer(turn, client, subscription_id).await? {
        Ok(offer) => offer,
        Err(message) => return Ok(Screen::notice(message).section(Section::Options)),
    };
    if offer.can_add == 0 {
        return Ok(Screen::notice(ui::text(c, "opt.max", &[])).section(Section::Options));
    }
    let stars = offer
        .monthly_stars
        .map(|s| ui::text(c, "opt.stars", &[("stars", Text(&format::stars(s)))]))
        .unwrap_or_default();
    let message = ui::text(
        c,
        "opt.offer",
        &[
            ("included", Text(&offer.included.to_string())),
            ("extra", Text(&offer.extra_now.to_string())),
            ("can_add", Text(&offer.can_add.to_string())),
            ("price", Text(&format::money(offer.monthly))),
            ("stars", Html(&stars)),
            ("months", Text(&offer.months.to_string())),
        ],
    );
    let buttons: Vec<_> = (1..=offer.can_add)
        .filter_map(|n| {
            ui::button(
                c,
                "opt.btn.count",
                &[("count", Text(&n.to_string()))],
                format!("opt:{subscription_id}:{n}"),
            )
        })
        .collect();
    Ok(Screen::new(message)
        .keyboard(Keyboard::new().grid(buttons, 4).push(ui::menu(c)))
        .section(Section::Options))
}

pub async fn options_action(
    turn: &mut Turn<'_>,
    client: &Client,
    rest: &str,
) -> Result<Screen, BotError> {
    if !turn.services().modules().is_enabled(ModuleId::Options) {
        return main_menu(turn.ctx, client).await;
    }
    let (sub, count) = rest.split_once(':').unwrap_or((rest, ""));
    let Some(sub) = id_arg(sub).map(SubscriptionId) else {
        return options_start(turn, client).await;
    };
    let Ok(count) = count.parse::<u32>() else {
        return options_offer(turn, client, sub).await;
    };
    let content = turn.services().bot_content();
    let c = &*content;
    let offer = match offer(turn, client, sub).await? {
        Ok(offer) => offer,
        Err(message) => return Ok(Screen::notice(message).section(Section::Options)),
    };
    let quote = match options::devices_quote(&offer, count) {
        Ok(quote) => quote,
        Err(OptionsError::Rejected(rejection)) => {
            return Ok(Screen::notice(rejection.message()).section(Section::Options));
        }
        Err(OptionsError::Module(err)) => return Ok(Screen::notice(module_error(err))),
    };
    let stars = quote
        .stars
        .map(|s| ui::text(c, "opt.stars", &[("stars", Text(&format::stars(s.total)))]))
        .unwrap_or_default();
    let label = quote
        .lines
        .first()
        .map(|line| line.label.clone())
        .unwrap_or_default();
    Ok(Screen::new(ui::text(
        c,
        "opt.quote",
        &[
            ("label", Text(&label)),
            ("total", Text(&format::money(quote.total))),
            ("stars", Html(&stars)),
        ],
    ))
    .keyboard(
        Keyboard::new()
            .push(ui::button(
                c,
                "opt.btn.extend_with",
                &[],
                format!("buy:{sub}"),
            ))
            .push(ui::menu(c)),
    )
    .section(Section::Options))
}
