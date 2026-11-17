use crate::BotError;
use crate::account::payment_screen;
use crate::handler::{Screen, Turn, id_arg, live, tariff_name};
use crate::session::{Awaiting, Cart};
use crate::telegram::{Button, Keyboard};
use crate::text::{self, escape};
use sn_domain::client::Client;
use sn_domain::format;
use sn_domain::ids::{PeriodId, TariffId};
use sn_domain::module::ModuleId;
use sn_domain::plan::PlanKind;
use sn_modules::catalog::promo::{self, PromoError, Purpose};
use sn_payments::Unavailable;
use sn_services::checkout::{CartRequest, Channel, CheckoutRequest, TariffOffer};
use sn_store::Db;

fn back_target(cart: &Cart) -> String {
    match cart.subscription_id {
        Some(id) => format!("buy:{id}"),
        None => "buy:new".into(),
    }
}

pub async fn start(turn: &mut Turn<'_>, client: &Client, rest: &str) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let subscriptions = live(services.subscriptions().list_for_client(client.id).await?);
    let target = if rest == "new" {
        None
    } else if let Some(id) = id_arg(rest) {
        subscriptions.iter().find(|s| s.id.0 == id).map(|s| s.id)
    } else {
        match subscriptions.as_slice() {
            [] => None,
            [only] => Some(only.id),
            many => {
                let offset = services.settings().remna_offset();
                let mut keyboard = Keyboard::new();
                for sub in many {
                    let name = tariff_name(&services, sub.tariff_id).await;
                    let label = match sub.mirror.expire_at {
                        Some(at) => format!("Продлить {name} · до {}", text::day_month(at, offset)),
                        None => format!("Продлить {name}"),
                    };
                    keyboard = keyboard.button(label, format!("buy:{}", sub.id));
                }
                return Ok(Screen::new("Какую подписку продлить?").keyboard(
                    keyboard
                        .button("Новая подписка", "buy:new")
                        .button("В меню", "m"),
                ));
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
    Ok(turn.services().checkout().catalog().await?)
}

async fn catalog(turn: &mut Turn<'_>) -> Result<Screen, BotError> {
    let offers = offers(turn).await?;
    if offers.is_empty() {
        return Ok(Screen::notice("Сейчас нет тарифов в продаже."));
    }
    let mut lines = vec!["<b>Выберите тариф</b>".to_string()];
    let mut keyboard = Keyboard::new();
    let mut category = None;
    for offer in &offers {
        if category != Some(offer.category.id) {
            category = Some(offer.category.id);
            lines.push(String::new());
            lines.push(format!("<b>{}</b>", escape(&offer.category.name)));
        }
        let cheapest = offer
            .periods
            .iter()
            .map(|p| p.per_month)
            .min_by_key(|m| m.minor);
        let price = cheapest
            .map(|m| format!(" — от {} в месяц", format::money(m)))
            .unwrap_or_default();
        lines.push(format!("{}{price}", escape(&offer.tariff.name)));
        keyboard = keyboard.button(offer.tariff.name.clone(), format!("t:{}", offer.tariff.id));
    }
    Ok(Screen::new(lines.join("\n")).keyboard(keyboard.button("В меню", "m")))
}

pub async fn tariff(
    turn: &mut Turn<'_>,
    client: &Client,
    tariff_id: TariffId,
) -> Result<Screen, BotError> {
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
    let mut lines = vec![format!("<b>{}</b>", escape(&tariff.name))];
    if let Some(description) = tariff
        .description
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        lines.push(escape(description));
    }
    lines.push(format!("Устройств в тарифе: {}", tariff.devices.included));
    match tariff.traffic.limit_bytes {
        Some(limit) if limit > 0 => lines.push(format!(
            "Трафик: {}, {}",
            format::traffic(limit),
            tariff.traffic.reset.short_ru()
        )),
        _ => lines.push("Трафик: без лимита".into()),
    }
    lines.push(String::new());
    lines.push("Выберите срок:".into());
    let mut keyboard = Keyboard::new();
    for period in &offer.periods {
        let mut label = format!(
            "{} — {}",
            format::days(period.days),
            format::money(period.price)
        );
        if period.months > 1 {
            label.push_str(&format!(", {} в месяц", format::money(period.per_month)));
        }
        if let Some(savings) = period.savings_percent {
            label.push_str(&format!(", выгоднее на {savings}%"));
        }
        keyboard = keyboard.button(label, format!("p:{}", period.period_id));
    }
    Ok(Screen::new(lines.join("\n")).keyboard(keyboard.row(vec![
        Button::callback("Назад", back),
        Button::callback("В меню", "m"),
    ])))
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
    let mut keyboard = Keyboard::new().button("Без доп. устройств", "d:0");
    let buttons: Vec<Button> = (1..=extra.max)
        .map(|n| Button::callback(format!("+{n}"), format!("d:{n}")))
        .collect();
    for chunk in buttons.chunks(4) {
        keyboard = keyboard.row(chunk.to_vec());
    }
    let included = tariff.devices.included;
    Ok(Screen::new(format!(
        "В тарифе {included}{}{}. Добавить ещё? Каждое — {} в месяц.",
        text::NB,
        format::plural_ru(u64::from(included), "устройство", "устройства", "устройств"),
        format::money(monthly)
    ))
    .keyboard(keyboard.button("Назад", format!("t:{tariff_id}"))))
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
    let mut notes = Vec::new();
    let view = match services.checkout().quote(client.id, &cart_request).await {
        Ok(view) => view,
        Err(err) if cart_request.promo_code.is_some() => {
            notes.push(format!("{} Промокод убран.", text::user_error(&err)));
            turn.session.promo = None;
            let plain = CartRequest {
                promo_code: None,
                ..cart_request.clone()
            };
            match services.checkout().quote(client.id, &plain).await {
                Ok(view) => view,
                Err(err) => return Ok(Screen::notice(text::user_error(&err))),
            }
        }
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    let offset = services.settings().remna_offset();
    let quote = &view.quote;
    let mut lines = vec!["<b>Оплата</b>".to_string()];
    for line in &quote.lines {
        lines.push(format!(
            "{} — {}",
            escape(&line.label),
            format::money(line.amount)
        ));
    }
    let stars = quote
        .stars
        .map(|s| format!(" (или {})", format::stars(s.total)))
        .unwrap_or_default();
    lines.push(format!(
        "К оплате: <b>{}</b>{stars}",
        format::money(quote.total)
    ));
    let until = text::date(view.expire_preview, offset);
    lines.push(match view.kind {
        PlanKind::Renewal => format!("Продление: подписка будет действовать до {until}."),
        PlanKind::Change => format!(
            "Смена тарифа: новый срок начнётся с даты оплаты, оставшиеся дни не переносятся. Подписка будет действовать до {until}."
        ),
        PlanKind::New => format!("Подписка будет действовать до {until}."),
    });
    let mut keyboard = Keyboard::new();
    for method in &view.methods {
        match (&method.unavailable, method.amount) {
            (None, Some(amount)) => {
                keyboard = keyboard.button(
                    format!("{} — {}", method.title, text::amount(amount)),
                    format!("pay:{}", method.provider),
                );
            }
            (Some(Unavailable::InsufficientBalance { balance, missing }), _) => {
                lines.push(format!(
                    "С баланса: не хватает {} (на балансе {})",
                    format::money(*missing),
                    format::money(*balance)
                ));
            }
            _ => {}
        }
    }
    if view.methods.iter().all(|m| m.unavailable.is_some()) {
        lines.push("Сейчас нет доступных способов оплаты.".into());
    }
    lines.extend(notes);
    if services.modules().is_enabled(ModuleId::Promo) {
        keyboard = match turn.session.promo {
            Some(_) => keyboard.button("Убрать промокод", "nopromo"),
            None => keyboard.button("Ввести промокод", "promo"),
        };
    }
    let back = match cart.tariff_id {
        Some(tariff) => format!("t:{tariff}"),
        None => back_target(&cart),
    };
    Ok(Screen::new(lines.join("\n")).keyboard(keyboard.row(vec![
        Button::callback("Назад", back),
        Button::callback("В меню", "m"),
    ])))
}

pub fn ask_promo(turn: &mut Turn<'_>) -> Screen {
    turn.session.wait_for(Awaiting::PromoForCart);
    Screen::new("Введите промокод:").keyboard(Keyboard::new().button("Отменить", "sum"))
}

pub async fn promo_entered(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
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
        Ok(_) => Ok(
            Screen::new(promo::PromoRejection::DaysCode.message()).keyboard(
                Keyboard::new()
                    .button("Активировать промокод", "x:promo")
                    .button("Вернуться к оплате", "sum"),
            ),
        ),
        Err(PromoError::Rejected(rejection)) => Ok(Screen::new(rejection.message()).keyboard(
            Keyboard::new()
                .button("Ввести другой", "promo")
                .button("Без промокода", "sum"),
        )),
        Err(PromoError::Module(err)) => Ok(Screen::notice(text::user_error(&err.into()))),
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
            Ok(payment_screen(&result, &format!("{}.", escape(&title))))
        }
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}
