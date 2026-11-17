use crate::BotError;
use crate::handler::{Screen, Turn, live, menu_keyboard, tariff_name};
use crate::session::{Awaiting, DevicePick};
use crate::telegram::{Button, Keyboard};
use crate::text::{self, escape};
use sn_domain::client::Client;
use sn_domain::format;
use sn_domain::ids::SubscriptionId;
use sn_domain::ledger::Wallet;
use sn_domain::money::{Currency, Money, PayCurrency, parse_units};
use sn_domain::pricing::topup_quote;
use sn_domain::support::TicketChannel;
use sn_domain::traffic::{NextReset, next_reset};
use sn_payments::{MethodsContext, ProviderKind};
use sn_services::checkout::{Channel, CheckoutResult};
use sn_services::support::OpenTicket;
use sn_services::{Actor, ServiceError};
use sn_store::{Db, StoreError};

pub fn payment_screen(result: &CheckoutResult, what: &str) -> Screen {
    match result {
        CheckoutResult::Redirect { pay_url, .. } if pay_url.starts_with("http") => {
            Screen::new(format!(
                "{what}\nСчёт готов: нажмите «Оплатить». После оплаты пришлём подтверждение сюда."
            ))
            .keyboard(
                Keyboard::new()
                    .link("Оплатить", pay_url.clone())
                    .button("В меню", "m"),
            )
        }
        CheckoutResult::Redirect { pay_url, .. } => {
            Screen::notice(format!("{what}\nСсылка на оплату: {}", escape(pay_url)))
        }
        CheckoutResult::Completed { .. } => {
            Screen::new("Оплачено с баланса. Подписка обновится в течение минуты.").keyboard(
                Keyboard::new()
                    .button("Подписка", "subs")
                    .button("В меню", "m"),
            )
        }
    }
}

pub async fn subscriptions(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let list = live(services.subscriptions().list_for_client(client.id).await?);
    match list.as_slice() {
        [] => Ok(Screen::new("Подписки пока нет.").keyboard(
            Keyboard::new()
                .button("Купить подписку", "buy")
                .button("В меню", "m"),
        )),
        [only] => subscription(turn, client, only.id.0).await,
        many => {
            let offset = services.settings().remna_offset();
            let mut keyboard = Keyboard::new();
            for sub in many {
                let name = tariff_name(&services, sub.tariff_id).await;
                let label = match sub.mirror.expire_at {
                    Some(at) => format!("{name} · до {}", text::day_month(at, offset)),
                    None => name,
                };
                keyboard = keyboard.button(label, format!("sub:{}", sub.id));
            }
            Ok(Screen::new("Выберите подписку:").keyboard(keyboard.button("В меню", "m")))
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
    let actor = Actor::Client(client.id);
    let sid = SubscriptionId(id);
    let (view, link_error) = match services.subscriptions().view(sid, actor, true).await {
        Ok(view) => (view, None),
        Err(err) if not_found(&err) => return Ok(Screen::notice("Подписка не найдена.")),
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
    let mut lines = vec![format!(
        "<b>{}</b> · {}",
        escape(&name),
        text::status_title(sub, now).to_lowercase()
    )];
    match sub.mirror.expire_at {
        Some(at) if at > now => lines.push(format!(
            "До {}, осталось {}",
            text::date(at, offset),
            text::plural_days(text::days_left(at, now))
        )),
        Some(at) => lines.push(format!("Закончилась {}", text::date(at, offset))),
        None => {}
    }
    match sub.mirror.traffic_limit_bytes.filter(|l| *l > 0) {
        Some(limit) => {
            let used = sub.mirror.traffic_used_bytes.unwrap_or(0);
            let mut line = format!(
                "Трафик: {} из {}",
                format::traffic(used),
                format::traffic(limit)
            );
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
            match reset {
                Some(NextReset::At(at)) => {
                    line.push_str(&format!(", обновится {}", text::day_month(at, offset)))
                }
                Some(NextReset::OnPayment) => line.push_str(", обновится при продлении"),
                _ => {}
            }
            lines.push(line);
        }
        None => lines.push("Трафик: без лимита".into()),
    }
    match (sub.mirror.devices_count, sub.mirror.hwid_limit) {
        (Some(count), Some(limit)) => lines.push(format!("Устройства: {count} из {limit}")),
        (None, Some(limit)) => lines.push(format!("Устройств можно подключить: {limit}")),
        _ => {}
    }
    lines.push(format!(
        "Автопродление: {}",
        if sub.autorenew.enabled {
            "включено"
        } else {
            "выключено"
        }
    ));
    let mut keyboard = Keyboard::new();
    match (&view.subscription_url, &link_error) {
        (Some(url), _) => {
            lines.push(String::new());
            lines.push("Ссылка подписки:".into());
            lines.push(format!("<code>{}</code>", escape(url)));
            if url.starts_with("http") {
                keyboard = keyboard.link("Открыть страницу подключения", url.clone());
            }
        }
        (None, Some(err)) => {
            lines.push(String::new());
            lines.push(format!(
                "Ссылку сейчас не получить. {}",
                text::user_error(err)
            ));
        }
        (None, None) => {}
    }
    if sub.remna.is_some() {
        keyboard = keyboard
            .button("Устройства", format!("dev:{id}"))
            .button("Выпустить новую ссылку", format!("rei:{id}"));
    }
    keyboard = keyboard.button("Продлить", format!("buy:{id}")).row(vec![
        Button::callback("Все подписки", "subs"),
        Button::callback("В меню", "m"),
    ]);
    Ok(Screen::new(lines.join("\n")).keyboard(keyboard))
}

pub async fn devices(turn: &mut Turn<'_>, client: &Client, id: i64) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let list = match services
        .subscriptions()
        .devices(SubscriptionId(id), Actor::Client(client.id))
        .await
    {
        Ok(list) => list,
        Err(err) => return Ok(Screen::notice(text::user_error(&err))),
    };
    let back = Keyboard::new().row(vec![
        Button::callback("Назад", format!("sub:{id}")),
        Button::callback("В меню", "m"),
    ]);
    if list.is_empty() {
        turn.session.devices = None;
        return Ok(Screen::new(
            "Устройств пока нет: они появятся, когда вы добавите подписку в приложение.",
        )
        .keyboard(back));
    }
    let offset = services.settings().remna_offset();
    let mut lines = vec!["<b>Устройства</b>".to_string()];
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
        let details = if details.is_empty() {
            String::new()
        } else {
            format!(" — {}", escape(&details.join(" · ")))
        };
        lines.push(format!(
            "{}. {}{details}, добавлено {}",
            index + 1,
            escape(&device.title),
            text::day_month(device.added_at, offset)
        ));
        keyboard = keyboard.button(format!("Отключить {}", device.title), format!("dx:{index}"));
    }
    turn.session.devices = Some(DevicePick {
        subscription_id: SubscriptionId(id),
        hwids: list.into_iter().map(|d| (d.hwid, d.title)).collect(),
    });
    keyboard.inline_keyboard.extend(back.inline_keyboard);
    Ok(Screen::new(lines.join("\n")).keyboard(keyboard))
}

fn picked(turn: &Turn<'_>, rest: &str) -> Option<(SubscriptionId, usize, String, String)> {
    let index: usize = rest.parse().ok()?;
    let pick = turn.session.devices.as_ref()?;
    let (hwid, title) = pick.hwids.get(index)?;
    Some((pick.subscription_id, index, hwid.clone(), title.clone()))
}

pub async fn device_confirm(turn: &mut Turn<'_>, rest: &str) -> Result<Screen, BotError> {
    let Some((sub, index, _, title)) = picked(turn, rest) else {
        return Ok(Screen::notice(
            "Список устройств устарел. Откройте его заново.",
        ));
    };
    Ok(Screen::new(format!(
        "Отключить {}? Место освободится, но если подписка на нём осталась, при обновлении он займёт его снова. \
         Чтобы закрыть доступ, выпустите новую ссылку.",
        escape(&title)
    ))
    .keyboard(
        Keyboard::new()
            .button("Отключить", format!("dxy:{index}"))
            .button("Отменить", format!("dev:{sub}")),
    ))
}

pub async fn device_disconnect(
    turn: &mut Turn<'_>,
    client: &Client,
    rest: &str,
) -> Result<Screen, BotError> {
    let Some((sub, _, hwid, title)) = picked(turn, rest) else {
        return Ok(Screen::notice(
            "Список устройств устарел. Откройте его заново.",
        ));
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
                Screen::new(format!("{} отключено.", escape(&title))).keyboard(
                    Keyboard::new()
                        .button("Устройства", format!("dev:{sub}"))
                        .button("В меню", "m"),
                ),
            )
        }
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}

pub fn reissue_confirm(id: i64) -> Screen {
    Screen::new(
        "Старая ссылка перестанет работать, а с ней все устройства. \
         Нужные устройства подключите заново по новой ссылке.",
    )
    .keyboard(
        Keyboard::new()
            .button("Выпустить новую ссылку", format!("reiy:{id}"))
            .button("Отменить", format!("sub:{id}")),
    )
}

pub async fn reissue(turn: &mut Turn<'_>, client: &Client, id: i64) -> Result<Screen, BotError> {
    match turn
        .services()
        .subscriptions()
        .reissue_link(SubscriptionId(id), Actor::Client(client.id))
        .await
    {
        Ok(url) => {
            let mut keyboard = Keyboard::new();
            if url.starts_with("http") {
                keyboard = keyboard.link("Открыть страницу подключения", url.clone());
            }
            Ok(Screen::new(format!(
                "Новая ссылка подписки:\n<code>{}</code>\nДобавьте её в приложение заново.",
                escape(&url)
            ))
            .keyboard(keyboard.button("В меню", "m")))
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
    let mut lines = vec![format!("На балансе: <b>{}</b>", format::money(main))];
    if referral.minor > 0 {
        lines.push(format!(
            "Реферальный баланс: {} (его можно вывести в разделе «Пригласить друга»)",
            format::money(referral)
        ));
    }
    lines.push("С баланса оплачивается вся сумма сразу, без доплаты картой.".into());
    let limits = settings.topup_limits();
    let buttons: Vec<Button> = presets(currency)
        .into_iter()
        .filter(|m| *m >= limits.min.minor && *m <= limits.max.minor)
        .map(|minor| {
            Button::callback(
                format!("+{}", format::money(Money::new(minor, currency))),
                format!("top:{minor}"),
            )
        })
        .collect();
    let mut keyboard = Keyboard::new();
    for pair in buttons.chunks(2) {
        keyboard = keyboard.row(pair.to_vec());
    }
    Ok(Screen::new(lines.join("\n")).keyboard(
        keyboard
            .button("Другая сумма", "topx")
            .button("В меню", "m"),
    ))
}

pub async fn topup_amount(turn: &mut Turn<'_>, minor: i64) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let settings = services.settings();
    let amount = Money::new(minor, settings.currency);
    if let Err(err) = topup_quote(amount, settings.topup_limits()) {
        turn.session.wait_for(Awaiting::TopUpAmount);
        return Ok(Screen::new(format!(
            "{} Введите другую сумму.",
            text::capitalize(&err.to_string())
        ))
        .keyboard(menu_keyboard()));
    }
    let methods: Vec<_> = services
        .payments()
        .checkout_methods(&MethodsContext {
            currency: settings.currency,
            total: amount,
            stars_total: None,
            balance: None,
        })
        .into_iter()
        .filter(|m| m.kind == ProviderKind::External && m.unavailable.is_none())
        .collect();
    if methods.is_empty() {
        return Ok(Screen::notice(
            "Пополнение сейчас недоступно: не подключена ни одна платёжная система.",
        ));
    }
    turn.session.topup_minor = Some(minor);
    let mut keyboard = Keyboard::new();
    for method in methods {
        let label = if method.hint.is_empty() {
            method.title.to_string()
        } else {
            format!("{} · {}", method.title, method.hint)
        };
        keyboard = keyboard.button(label, format!("tp:{}", method.provider));
    }
    Ok(Screen::new(format!(
        "Пополнение на {}. Выберите способ оплаты:",
        format::money(amount)
    ))
    .keyboard(keyboard.button("Назад", "bal")))
}

pub fn topup_custom(turn: &mut Turn<'_>) -> Screen {
    turn.session.wait_for(Awaiting::TopUpAmount);
    let example = match turn.services().settings().currency {
        Currency::Usd => "например 10 или 12.50",
        Currency::Rub => "например 500",
    };
    Screen::new(format!("Введите сумму пополнения, {example}."))
        .keyboard(Keyboard::new().button("Отменить", "bal"))
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
            Ok(Screen::new("Не понял сумму. Введите число, например 10.")
                .keyboard(Keyboard::new().button("Отменить", "bal")))
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
    let amount = Money::new(minor, services.settings().currency);
    match services
        .checkout()
        .topup(client.id, amount, provider, Channel::Bot)
        .await
    {
        Ok(result) => {
            turn.session.topup_minor = None;
            Ok(payment_screen(
                &result,
                &format!("Пополнение на {}.", format::money(amount)),
            ))
        }
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}

pub fn support_prompt(turn: &mut Turn<'_>) -> Screen {
    turn.session.wait_for(Awaiting::SupportMessage);
    let mut keyboard = Keyboard::new();
    if let Some(url) = turn
        .services()
        .settings()
        .support_url
        .as_deref()
        .filter(|u| u.starts_with("http"))
    {
        keyboard = keyboard.link("Другие способы связи", url);
    }
    Screen::new("Опишите вопрос одним сообщением — ответим здесь, в боте.")
        .keyboard(keyboard.button("Отменить", "m"))
}

pub async fn support_entered(
    turn: &mut Turn<'_>,
    client: &Client,
    body: &str,
) -> Result<Screen, BotError> {
    if body.trim().is_empty() {
        return Ok(support_prompt(turn));
    }
    match turn
        .services()
        .support()
        .open(
            client.id,
            &OpenTicket {
                subscription_id: None,
                channel: TicketChannel::Bot,
                subject: None,
                body: body.to_string(),
            },
        )
        .await
    {
        Ok(id) => Ok(Screen::notice(format!(
            "Обращение №{id} отправлено. Ответим здесь, в боте."
        ))),
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}

pub async fn trial(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    match turn.services().checkout().start_trial(client.id).await {
        Ok(_) => Ok(Screen::new(
            "Пробный период оформляется. Через минуту подписка появится в разделе «Подписка».",
        )
        .keyboard(
            Keyboard::new()
                .button("Подписка", "subs")
                .button("В меню", "m"),
        )),
        Err(err) => Ok(Screen::notice(text::user_error(&err))),
    }
}
