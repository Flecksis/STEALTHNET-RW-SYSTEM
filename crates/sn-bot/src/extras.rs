use crate::BotError;
use crate::handler::{Screen, Turn, id_arg, live, main_menu, tariff_name};
use crate::session::{Awaiting, WithdrawalDraft};
use crate::telegram::{Button, Keyboard};
use crate::text::{self, escape};
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
    Screen::new(
        "Введите промокод. Скидку применим при оплате, а бесплатные дни сразу добавим к подписке.",
    )
    .keyboard(Keyboard::new().button("Отменить", "m"))
}

pub async fn promo_entered(
    turn: &mut Turn<'_>,
    client: &Client,
    raw: &str,
) -> Result<Screen, BotError> {
    let services = turn.services().clone();
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
            return Ok(Screen::new(rejection.message()).keyboard(
                Keyboard::new()
                    .button("Ввести другой", format!("x:{}", promo::BOT_ENTRY))
                    .button("В меню", "m"),
            ));
        }
        Err(PromoError::Module(err)) => return Ok(Screen::notice(module_error(err))),
    };
    if code.is_discount() {
        let label = code.label();
        turn.session.promo = Some(code.code);
        return Ok(Screen::new(format!(
            "{} сохранён: скидка применится при оплате.",
            escape(&label)
        ))
        .keyboard(
            Keyboard::new()
                .button("Выбрать тариф", "buy")
                .button("В меню", "m"),
        ));
    }
    let subscriptions: Vec<_> = live(services.subscriptions().list_for_client(client.id).await?)
        .into_iter()
        .filter(|s| s.remna.is_some())
        .collect();
    match subscriptions.as_slice() {
        [] => Ok(Screen::notice(PromoRejection::NoSubscription.message())),
        [only] => {
            turn.session.days_code = Some(code.code);
            activate_days(turn, client, only.id.0).await
        }
        many => {
            turn.session.days_code = Some(code.code);
            let mut keyboard = Keyboard::new();
            for sub in many {
                let name = tariff_name(&services, sub.tariff_id).await;
                keyboard = keyboard.button(name, format!("pc:{}", sub.id));
            }
            Ok(Screen::new("К какой подписке добавить дни?")
                .keyboard(keyboard.button("Отменить", "m")))
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
    let ctx = turn.services().module_context();
    match promo::activate(&ctx, client.id, &code, SubscriptionId(subscription_id)).await {
        Ok(done) => Ok(
            Screen::new(format!("Добавили {} к подписке.", format::days(done.days))).keyboard(
                Keyboard::new()
                    .button("Подписка", format!("sub:{subscription_id}"))
                    .button("В меню", "m"),
            ),
        ),
        Err(PromoError::Rejected(rejection)) => Ok(Screen::notice(rejection.message())),
        Err(PromoError::Module(err)) => Ok(Screen::notice(module_error(err))),
    }
}

async fn referral_screen(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let ctx = turn.services().module_context();
    let overview = referral::overview(&ctx, client.id).await?;
    let link = overview.bot_link.clone().or_else(|| {
        turn.ctx
            .username
            .as_deref()
            .map(|username| referral::invite_link(username, &overview.code))
    });
    let mut lines = vec![
        "<b>Пригласить друга</b>".to_string(),
        format!(
            "Делитесь ссылкой: вы получаете {}% с каждой оплаты друга.",
            overview.levels[0]
        ),
    ];
    if let Some(link) = &link {
        lines.push(format!("<code>{}</code>", escape(link)));
    }
    lines.push(String::new());
    lines.push(format!("Приглашено: {}", overview.invited));
    lines.push(format!("Заработано: {}", format::money(overview.earned)));
    lines.push(format!(
        "На реферальном балансе: {}",
        format::money(overview.balance)
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
        lines.push(String::new());
        for withdrawal in &pending {
            lines.push(format!(
                "Заявка №{} на {}: {}",
                withdrawal.id,
                format::money(withdrawal.amount),
                withdrawal.status.title_ru().to_lowercase()
            ));
            if withdrawal.status == WithdrawalStatus::Pending {
                keyboard = keyboard.button(
                    format!("Отменить заявку №{}", withdrawal.id),
                    format!("ref:c:{}", withdrawal.id),
                );
            }
        }
    }
    if overview.withdrawals_enabled && overview.balance.minor > 0 {
        keyboard = keyboard.button("Вывести деньги", "ref:w");
    }
    if let Some(link) = link {
        keyboard = keyboard.link(
            "Поделиться ссылкой",
            format!("https://t.me/share/url?url={}", text::url_encode(&link)),
        );
    }
    Ok(Screen::new(lines.join("\n")).keyboard(keyboard.button("В меню", "m")))
}

pub async fn referral_action(
    turn: &mut Turn<'_>,
    client: &Client,
    rest: &str,
) -> Result<Screen, BotError> {
    if !turn.services().modules().is_enabled(ModuleId::Referral) {
        return main_menu(turn.ctx, client).await;
    }
    let (action, arg) = rest.split_once(':').unwrap_or((rest, ""));
    match action {
        "w" => {
            let buttons: Vec<Button> = PayoutMethod::ALL
                .into_iter()
                .map(|m| Button::callback(m.title_ru(), format!("ref:m:{}", m.as_str())))
                .collect();
            let mut keyboard = Keyboard::new();
            for button in buttons {
                keyboard = keyboard.row(vec![button]);
            }
            Ok(Screen::new("Как получить деньги?")
                .keyboard(keyboard.button("Отменить", format!("x:{}", referral::BOT_ENTRY))))
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
                .map(|m| format!(" Минимум — {}.", format::money(m)))
                .unwrap_or_default();
            Ok(Screen::new(format!(
                "Сколько вывести? На реферальном балансе {}.{minimum}",
                format::money(overview.balance)
            ))
            .keyboard(Keyboard::new().button("Отменить", format!("x:{}", referral::BOT_ENTRY))))
        }
        "c" => {
            let Some(id) = id_arg(arg) else {
                return referral_screen(turn, client).await;
            };
            let ctx = turn.services().module_context();
            match referral::cancel_withdrawal(&ctx, client.id, id).await {
                Ok(withdrawal) => Ok(Screen::new(format!(
                    "Заявка №{} отменена, {} вернулись на реферальный баланс.",
                    withdrawal.id,
                    format::money(withdrawal.amount)
                ))
                .keyboard(Keyboard::new().button("Назад", format!("x:{}", referral::BOT_ENTRY)))),
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
    Ok(
        Screen::new(format!("Укажите реквизиты. {}", method.details_hint()))
            .keyboard(Keyboard::new().button("Отменить", format!("x:{}", referral::BOT_ENTRY))),
    )
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
        Ok(withdrawal) => Ok(Screen::new(format!(
            "Заявка №{} на {} принята. Сумма списана с реферального баланса и вернётся, если заявку отклонят.",
            withdrawal.id,
            format::money(withdrawal.amount)
        ))
        .keyboard(Keyboard::new().button("Назад", format!("x:{}", referral::BOT_ENTRY)))),
        Err(err) => Ok(Screen::new(referral_error(err)).keyboard(
            Keyboard::new()
                .button("Попробовать снова", "ref:w")
                .button("В меню", "m"),
        )),
    }
}

async fn options_start(turn: &mut Turn<'_>, client: &Client) -> Result<Screen, BotError> {
    let services = turn.services().clone();
    let subscriptions: Vec<_> = live(services.subscriptions().list_for_client(client.id).await?)
        .into_iter()
        .filter(|s| s.remna.is_some())
        .collect();
    match subscriptions.as_slice() {
        [] => Ok(Screen::notice(
            "Докупить устройства можно к действующей подписке.",
        )),
        [only] => options_offer(turn, client, only.id).await,
        many => {
            let mut keyboard = Keyboard::new();
            for sub in many {
                let name = tariff_name(&services, sub.tariff_id).await;
                keyboard = keyboard.button(name, format!("opt:{}", sub.id));
            }
            Ok(Screen::new("К какой подписке добавить устройства?")
                .keyboard(keyboard.button("В меню", "m")))
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
    let offer = match offer(turn, client, subscription_id).await? {
        Ok(offer) => offer,
        Err(message) => return Ok(Screen::notice(message)),
    };
    if offer.can_add == 0 {
        return Ok(Screen::notice(
            "К этой подписке больше устройств не докупить.",
        ));
    }
    let stars = offer
        .monthly_stars
        .map(|s| format!(" или {}", format::stars(s)))
        .unwrap_or_default();
    let text = format!(
        "Сейчас: {} в тарифе и {} докуплено. Можно добавить ещё {}.\nЦена: {}{stars} в месяц за устройство, до конца подписки — {} мес.",
        offer.included,
        offer.extra_now,
        offer.can_add,
        format::money(offer.monthly),
        offer.months
    );
    let buttons: Vec<Button> = (1..=offer.can_add)
        .map(|n| Button::callback(format!("+{n}"), format!("opt:{subscription_id}:{n}")))
        .collect();
    let mut keyboard = Keyboard::new();
    for chunk in buttons.chunks(4) {
        keyboard = keyboard.row(chunk.to_vec());
    }
    Ok(Screen::new(text).keyboard(keyboard.button("В меню", "m")))
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
    let offer = match offer(turn, client, sub).await? {
        Ok(offer) => offer,
        Err(message) => return Ok(Screen::notice(message)),
    };
    let quote = match options::devices_quote(&offer, count) {
        Ok(quote) => quote,
        Err(OptionsError::Rejected(rejection)) => return Ok(Screen::notice(rejection.message())),
        Err(OptionsError::Module(err)) => return Ok(Screen::notice(module_error(err))),
    };
    let stars = quote
        .stars
        .map(|s| format!(" или {}", format::stars(s.total)))
        .unwrap_or_default();
    let label = quote
        .lines
        .first()
        .map(|line| line.label.clone())
        .unwrap_or_default();
    Ok(Screen::new(format!(
        "{} — {}{stars}.\nОплата доп. устройств в боте появится позже. Сейчас их можно добавить при продлении подписки.",
        escape(&label),
        format::money(quote.total)
    ))
    .keyboard(
        Keyboard::new()
            .button("Продлить с устройствами", format!("buy:{sub}"))
            .button("В меню", "m"),
    ))
}
