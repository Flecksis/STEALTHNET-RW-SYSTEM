use crate::BotError;
use crate::account::payment_screen;
use crate::gram_wallet;
use crate::handler::{Screen, Turn, id_arg, live, tariff_name};
use crate::session::{Awaiting, Cart};
use crate::telegram::Keyboard;
use crate::text;
use crate::ui::{self, Html, Lines, Text};
use sn_domain::bot_content::Section;
use sn_domain::client::Client;
use sn_domain::format;
use sn_domain::ids::{PeriodId, TariffId};
use sn_domain::module::ModuleId;
use sn_domain::payment::providers;
use sn_domain::plan::PlanKind;
use sn_modules::catalog::promo::{self, PromoError, Purpose};
use sn_payments::Unavailable;
use sn_services::checkout::{CartRequest, Channel, CheckoutRequest, CheckoutResult, TariffOffer};
use sn_store::Db;
use uuid::Uuid;

const CURRENT_FALLBACK: &str = "без тарифа";

fn back_target(cart: &Cart) -> String {
    match cart.subscription_id {
        Some(id) => format!("buy:{id}"),
        None => "buy:new".into(),
    }
}

pub async fn start(turn: &mut Turn<'_>, client: &Client, rest: &str) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let multi = services.settings().multi_subscriptions;
    let subscriptions = live(services.subscriptions().list_for_client(client.id).await?);
    let target = if rest == "new" && (multi || subscriptions.is_empty()) {
        None
    } else if let Some(id) = id_arg(rest) {
        subscriptions.iter().find(|s| s.id.0 == id).map(|s| s.id)
    } else {
        match subscriptions.as_slice() {
            [] => None,
            [only] if !multi => Some(only.id),
            many => {
                let content = services.bot_content();
                let c = &*content;
                let offset = services.settings().remna_offset();
                let mut keyboard = Keyboard::new();
                for sub in many {
                    let name = tariff_name(&services, sub.tariff_id).await;
                    let data = format!("buy:{}", sub.id);
                    keyboard = keyboard.push(match sub.mirror.expire_at {
                        Some(at) => ui::button(
                            c,
                            "buy.btn.extend_item",
                            &[
                                ("tariff", Text(&name)),
                                ("date", Text(&text::day_month(at, offset))),
                            ],
                            data,
                        ),
                        None => ui::button(
                            c,
                            "buy.btn.extend_item_open",
                            &[("tariff", Text(&name))],
                            data,
                        ),
                    });
                }
                if multi {
                    keyboard = keyboard.push(ui::button(c, "buy.btn.new", &[], "buy:new"));
                }
                return Ok(Screen::new(ui::text(c, "buy.which", &[]))
                    .keyboard(keyboard.push(ui::menu(c)))
                    .section(Section::Buy));
            }
        }
    };
    turn.session.cart = Some(Cart {
        subscription_id: target,
        ..Cart::default()
    });
    catalog(turn).await
}

async fn offers(turn: &Turn<'_>) -> Result<Vec<TariffOffer>, BotError> {
    Ok(turn
        .services()
        .checkout()
        .catalog_in(Some(sn_domain::access::ClientChannel::Bot))
        .await?)
}

async fn catalog(turn: &mut Turn<'_>) -> Result<Screen, BotError> {
    let content = turn.services().bot_content();
    let c = &*content;
    let offers = offers(turn).await?;
    if offers.is_empty() {
        return Ok(Screen::notice(ui::text(c, "buy.empty", &[])).section(Section::Buy));
    }
    let mut lines = Lines::new();
    lines.add(ui::text(c, "buy.title", &[]));
    let mut keyboard = Keyboard::new();
    let mut category = None;
    for offer in &offers {
        if category != Some(offer.category.id) {
            category = Some(offer.category.id);
            lines.add(ui::text(
                c,
                "buy.category",
                &[("category", Text(&offer.category.name))],
            ));
        }
        let cheapest = offer
            .periods
            .iter()
            .map(|p| p.per_month)
            .min_by_key(|m| m.minor);
        lines.add(match cheapest {
            Some(per_month) => ui::text(
                c,
                "buy.tariff_line",
                &[
                    ("tariff", Text(&offer.tariff.name)),
                    ("per_month", Text(&format::money(per_month))),
                ],
            ),
            None => ui::text(
                c,
                "buy.tariff_line_plain",
                &[("tariff", Text(&offer.tariff.name))],
            ),
        });
        keyboard = keyboard.push(ui::button(
            c,
            "buy.btn.tariff",
            &[("tariff", Text(&offer.tariff.name))],
            format!("t:{}", offer.tariff.id),
        ));
    }
    Ok(Screen::new(lines.text())
        .keyboard(keyboard.push(ui::menu(c)))
        .section(Section::Buy))
}

pub async fn tariff(
    turn: &mut Turn<'_>,
    client: &Client,
    tariff_id: TariffId,
) -> Result<Screen, BotError> {
    let content = turn.services().bot_content();
    let c = &*content;
    let offers = offers(turn).await?;
    let Some(offer) = offers.into_iter().find(|o| o.tariff.id == tariff_id) else {
        return start(turn, client, "").await;
    };
    let cart = turn.session.cart.get_or_insert_with(Cart::default);
    cart.tariff_id = Some(tariff_id);
    cart.period_id = None;
    cart.extra_devices = 0;
    let back = back_target(cart);
    let tariff = &offer.tariff;
    let mut lines = Lines::new();
    lines.add(ui::text(
        c,
        "buy.tariff_title",
        &[("tariff", Text(&tariff.name))],
    ));
    if let Some(description) = tariff
        .description
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        lines.add(text::escape(description));
    }
    lines.add(ui::text(
        c,
        "buy.tariff_devices",
        &[("count", Text(&tariff.devices.included.to_string()))],
    ));
    match tariff.traffic.limit_bytes {
        Some(limit) if limit > 0 => lines.add(ui::text(
            c,
            "buy.tariff_traffic",
            &[
                ("limit", Text(&format::traffic(limit))),
                ("reset", Text(tariff.traffic.reset.short_ru())),
            ],
        )),
        _ => lines.add(ui::text(c, "buy.tariff_unlimited", &[])),
    }
    lines.add(ui::text(c, "buy.choose_period", &[]));
    let mut keyboard = Keyboard::new();
    for period in &offer.periods {
        let days = format::days(period.days);
        let price = format::money(period.price);
        let per_month = format::money(period.per_month);
        let data = format!("p:{}", period.period_id);
        keyboard = keyboard.push(match period.savings_percent {
            Some(savings) => ui::button(
                c,
                "buy.btn.period_savings",
                &[
                    ("days", Text(&days)),
                    ("price", Text(&price)),
                    ("per_month", Text(&per_month)),
                    ("savings", Text(&savings.to_string())),
                ],
                data,
            ),
            None if period.months > 1 => ui::button(
                c,
                "buy.btn.period_monthly",
                &[
                    ("days", Text(&days)),
                    ("price", Text(&price)),
                    ("per_month", Text(&per_month)),
                ],
                data,
            ),
            None => ui::button(
                c,
                "buy.btn.period",
                &[("days", Text(&days)), ("price", Text(&price))],
                data,
            ),
        });
    }
    Ok(Screen::new(lines.text())
        .keyboard(keyboard.pair(ui::back(c, back), ui::menu(c)))
        .section(Section::Buy))
}

pub async fn period(
    turn: &mut Turn<'_>,
    client: &Client,
    period_id: i64,
) -> Result<Screen, BotError> {
    let Some(tariff_id) = turn.session.cart.as_ref().and_then(|c| c.tariff_id) else {
        return start(turn, client, "").await;
    };
    let services = turn.services().clone();
    let content = services.bot_content();
    let c = &*content;
    let tariff = {
        let mut conn = services.store().conn().await?;
        conn.tariffs().get(tariff_id).await?
    };
    if tariff.period(PeriodId(period_id)).is_none() {
        return self::tariff(turn, client, tariff_id).await;
    }
    if let Some(cart) = turn.session.cart.as_mut() {
        cart.period_id = Some(PeriodId(period_id));
        cart.extra_devices = 0;
    }
    let currency = services.settings().currency;
    let extra = tariff
        .devices
        .extra
        .as_ref()
        .filter(|e| e.max > 0 && services.modules().is_enabled(ModuleId::Options));
    let Some(extra) = extra else {
        return summary(turn, client).await;
    };
    let Some(monthly) = extra.monthly.get(currency) else {
        return summary(turn, client).await;
    };
    let buttons: Vec<_> = (1..=extra.max)
        .filter_map(|n| {
            ui::button(
                c,
                "buy.btn.extra",
                &[("count", Text(&n.to_string()))],
                format!("d:{n}"),
            )
        })
        .collect();
    let included = tariff.devices.included;
    let included_text = format!(
        "{included}{}{}",
        text::NB,
        format::plural_ru(u64::from(included), "устройство", "устройства", "устройств")
    );
    Ok(Screen::new(ui::text(
        c,
        "buy.extra_devices",
        &[
            ("included", Text(&included_text)),
            ("price", Text(&format::money(monthly))),
        ],
    ))
    .keyboard(
        Keyboard::new()
            .push(ui::button(c, "buy.btn.no_extra", &[], "d:0"))
            .grid(buttons, 4)
            .push(ui::back(c, format!("t:{tariff_id}"))),
    )
    .section(Section::Buy))
}

pub async fn devices(turn: &mut Turn<'_>, client: &Client, count: u32) -> Result<Screen, BotError> {
    if let Some(cart) = turn.session.cart.as_mut() {
        cart.extra_devices = count;
    }
    summary(turn, client).await
}

fn request(cart: &Cart, promo_code: Option<String>) -> Option<CartRequest> {
    Some(CartRequest {
        tariff_id: cart.tariff_id?,
        period_id: cart.period_id?,
        extra_devices: cart.extra_devices,
        subscription_id: cart.subscription_id,
        promo_code,
    })
}

pub async fn summary(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let Some(cart) = turn.session.cart.clone() else {
        return start(turn, client, "").await;
    };
    let Some(cart_request) = request(&cart, turn.session.promo.clone()) else {
        return catalog(turn).await;
    };
    let services = turn.services().clone();
    let content = services.bot_content();
    let c = &*content;
    let mut notes = Vec::new();
    let view = match services
        .checkout()
        .quote_in(client.id, &cart_request, Channel::Bot)
        .await
    {
        Ok(view) => view,
        Err(err) if cart_request.promo_code.is_some() => {
            notes.push(ui::text(
                c,
                "buy.promo_removed",
                &[("error", Text(&text::user_error(&err)))],
            ));
            turn.session.promo = None;
            let plain = CartRequest {
                promo_code: None,
                ..cart_request.clone()
            };
            match services
                .checkout()
                .quote_in(client.id, &plain, Channel::Bot)
                .await
            {
                Ok(view) => view,
                Err(err) => return Ok(Screen::notice(text::user_error(&err))),
            }
        }
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    let offset = services.settings().remna_offset();
    let quote = &view.quote;
    let until = text::date(view.expire_preview, offset);
    let tariff = tariff_name(&services, cart.tariff_id).await;
    let current = view
        .current
        .as_ref()
        .and_then(|current| current.tariff.clone())
        .unwrap_or_else(|| CURRENT_FALLBACK.to_string());
    let plan_text = ui::text(
        c,
        match (view.kind, view.current.as_ref().is_some_and(|s| s.trial)) {
            (PlanKind::Renewal, _) => "buy.summary_renewal",
            (PlanKind::Change, true) => "buy.summary_from_trial",
            (PlanKind::Change, false) => "buy.summary_change",
            (PlanKind::New, _) => "buy.summary_new",
        },
        &[
            ("date", Text(&until)),
            ("tariff", Text(&tariff)),
            ("current", Text(&current)),
        ],
    );
    let (before, after) = match view.kind {
        PlanKind::Change => (plan_text, String::new()),
        PlanKind::New | PlanKind::Renewal => (String::new(), plan_text),
    };
    let mut lines = Lines::new();
    lines.add(ui::text(c, "buy.summary_title", &[]));
    lines.add(before);
    for line in &quote.lines {
        lines.add(ui::text(
            c,
            "buy.summary_line",
            &[
                ("label", Text(&line.label)),
                ("amount", Text(&format::money(line.amount))),
            ],
        ));
    }
    let total = format::money(quote.total);
    lines.add(match quote.stars {
        Some(stars) => ui::text(
            c,
            "buy.summary_total_stars",
            &[
                ("total", Text(&total)),
                ("stars", Text(&format::stars(stars.total))),
            ],
        ),
        None => ui::text(c, "buy.summary_total", &[("total", Text(&total))]),
    });
    lines.add(after);
    let mut keyboard = Keyboard::new();
    for method in &view.methods {
        match (&method.unavailable, method.amount) {
            (None, Some(amount)) => {
                keyboard = keyboard.push(ui::button(
                    c,
                    "buy.btn.method",
                    &[
                        ("method", Text(method.title)),
                        ("amount", Text(&text::amount(amount))),
                    ],
                    format!("pay:{}", method.provider),
                ));
            }
            (Some(Unavailable::InsufficientBalance { balance, missing }), _) => {
                lines.add(ui::text(
                    c,
                    "buy.balance_short",
                    &[
                        ("missing", Text(&format::money(*missing))),
                        ("balance", Text(&format::money(*balance))),
                    ],
                ));
            }
            _ => {}
        }
    }
    if view.methods.iter().all(|m| m.unavailable.is_some()) {
        lines.add(ui::text(c, "buy.no_methods", &[]));
    }
    for note in notes {
        lines.add(note);
    }
    if services.modules().is_enabled(ModuleId::Promo) {
        keyboard = keyboard.push(match turn.session.promo {
            Some(_) => ui::button(c, "buy.btn.promo_remove", &[], "nopromo"),
            None => ui::button(c, "buy.btn.promo", &[], "promo"),
        });
    }
    let back = match cart.tariff_id {
        Some(tariff) => format!("t:{tariff}"),
        None => back_target(&cart),
    };
    Ok(Screen::new(lines.text())
        .keyboard(keyboard.pair(ui::back(c, back), ui::menu(c)))
        .section(Section::Buy))
}

pub fn ask_promo(turn: &mut Turn<'_>) -> Screen {
    turn.session.wait_for(Awaiting::PromoForCart);
    let content = turn.services().bot_content();
    Screen::new(ui::text(&content, "buy.ask_promo", &[]))
        .keyboard(Keyboard::new().push(ui::cancel(&content, "sum")))
        .section(Section::Buy)
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
    let tariff = turn.session.cart.as_ref().and_then(|c| c.tariff_id);
    match promo::check(
        &ctx,
        client.id,
        raw,
        Purpose::Check {
            tariff,
            currency: services.settings().currency,
        },
    )
    .await
    {
        Ok(code) if code.is_discount() => {
            turn.session.promo = Some(code.code);
            summary(turn, client).await
        }
        Ok(_) => Ok(Screen::new(promo::PromoRejection::DaysCode.message())
            .keyboard(
                Keyboard::new()
                    .push(ui::button(c, "buy.btn.promo_activate", &[], "x:promo"))
                    .push(ui::button(c, "buy.btn.back_to_payment", &[], "sum")),
            )
            .section(Section::Buy)),
        Err(PromoError::Rejected(rejection)) => Ok(Screen::new(rejection.message())
            .keyboard(
                Keyboard::new()
                    .push(ui::button(c, "buy.btn.promo_other", &[], "promo"))
                    .push(ui::button(c, "buy.btn.no_promo", &[], "sum")),
            )
            .section(Section::Buy)),
        Err(PromoError::Module(err)) => Ok(Screen::notice(text::user_error(&err.into()))),
    }
}

pub(crate) async fn send_gram_widget(
    turn: &Turn<'_>,
    payment_id: Uuid,
    what: &str,
) -> Result<bool, BotError> {
    let services = turn.services();
    let payment = {
        let mut conn = services.store().conn().await?;
        conn.payments().by_public_id(payment_id).await?
    };
    let Some(widget) = gram_wallet::compose(services, &payment, what) else {
        return Ok(false);
    };
    match turn.ctx.api.send_widget(turn.target.chat_id, &widget).await {
        Ok(_) => Ok(true),
        Err(err) => {
            tracing::warn!(error = %err, "виджет оплаты GRAM не отправлен, показываем ссылку");
            Ok(false)
        }
    }
}

pub async fn pay(turn: &mut Turn<'_>, client: &Client, provider: &str) -> Result<Screen, BotError> {
    let Some(cart) = turn.session.cart.clone() else {
        return start(turn, client, "").await;
    };
    let Some(cart_request) = request(&cart, turn.session.promo.clone()) else {
        return catalog(turn).await;
    };
    let services = turn.services().clone();
    let content = services.bot_content();
    let title = tariff_name(&services, cart.tariff_id).await;
    let result = services
        .checkout()
        .start(
            client.id,
            &CheckoutRequest {
                cart: cart_request,
                provider: provider.to_string(),
                channel: Channel::Bot,
                return_url: None,
            },
        )
        .await;
    match result {
        Ok(result) => {
            turn.session.cart = None;
            turn.session.promo = None;
            let what = ui::text(&content, "pay.what_tariff", &[("tariff", Text(&title))]);
            if let CheckoutResult::Redirect { payment_id, .. } = &result
                && provider == providers::GRAM
                && send_gram_widget(turn, *payment_id, &what).await?
            {
                return Ok(Screen::new(ui::text(
                    &content,
                    "pay.gram_ready",
                    &[("what", Html(&what))],
                ))
                .keyboard(Keyboard::new().push(ui::menu(&content)))
                .section(Section::Payment));
            }
            Ok(payment_screen(&content, &result, &what))
        }
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}
