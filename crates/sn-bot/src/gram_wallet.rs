use crate::BotError;
use crate::handler::BotCtx;
use crate::telegram::{Keyboard, WidgetMessage, WidgetQuery};
use crate::text;
use crate::ui::{self, Html, Text};
use serde_json::{Value, json};
use sn_domain::client::IdentityKind;
use sn_domain::lang;
use sn_domain::payment::{Payment, PaymentStatus, providers};
use sn_payments::providers::GramInvoice;
use sn_services::Services;
use sn_store::Db;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const HTML: &str = include_str!("gram_wallet.html");
pub const TEMPLATE: &str = sn_services::checkout::GRAM_WIDGET_TEMPLATE;
const PAY: &str = "p:";
const SENT: &str = "s:";
const VALID_SECONDS: i64 = 300;
const CHECK_WINDOW: Duration = Duration::from_secs(11);
const CHECK_PAUSE: Duration = Duration::from_millis(2_500);
const SENT_MEMORY: Duration = Duration::from_secs(900);

static SENT_AT: LazyLock<Mutex<HashMap<Uuid, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn sent_at() -> std::sync::MutexGuard<'static, HashMap<Uuid, Instant>> {
    match SENT_AT.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn remember_sent(payment: Uuid) {
    let mut map = sent_at();
    map.retain(|_, at| at.elapsed() < SENT_MEMORY);
    map.insert(payment, Instant::now());
}

fn recently_sent(payment: Uuid) -> bool {
    sent_at()
        .get(&payment)
        .is_some_and(|at| at.elapsed() < SENT_MEMORY)
}

pub fn available(services: &Services) -> bool {
    !services.payments().config(providers::GRAM).flag("testnet")
}

fn invoice(payment: &Payment) -> Option<GramInvoice> {
    if payment.provider.as_str() != providers::GRAM {
        return None;
    }
    GramInvoice::parse(payment.provider_txid.as_deref()?)
}

fn chat_data(payment: &Payment, invoice: &GramInvoice, state: &str) -> String {
    let fiat = invoice
        .amount
        .as_money()
        .map(|money| format!("≈ {}", text::money(money)))
        .unwrap_or_default();
    json!({
        "id": payment.public_id.simple().to_string(),
        "a": format!("{} GRAM", invoice.grams()),
        "f": fiat,
        "s": state,
    })
    .to_string()
}

pub fn compose(services: &Services, payment: &Payment, what: &str) -> Option<WidgetMessage> {
    if payment.status != PaymentStatus::Pending || !available(services) {
        return None;
    }
    let invoice = invoice(payment)?;
    invoice.wallet_transaction(0)?;
    let content = services.bot_content_in(lang::current());
    let grams = format!("{} GRAM", invoice.grams());
    let caption = ui::text(
        &content,
        "pay.gram_caption",
        &[("amount", Text(&grams)), ("what", Html(what))],
    );
    let keyboard = match payment.pay_url.as_deref().filter(|u| u.starts_with("http")) {
        Some(url) => Keyboard::new().push(ui::link(&content, "pay.btn.gram_other", &[], url)),
        None => Keyboard::new(),
    };
    Some(WidgetMessage {
        html: HTML.to_string(),
        chat_data: chat_data(payment, &invoice, ""),
        caption,
        keyboard,
    })
}

pub async fn compose_for(
    services: &Services,
    payment_id: Uuid,
) -> Result<Option<WidgetMessage>, BotError> {
    let (payment, tariff) = {
        let mut conn = services.store().conn().await?;
        let payment = conn.payments().by_public_id(payment_id).await?;
        let tariff = conn
            .orders()
            .get(payment.order_id)
            .await?
            .snapshot
            .map(|snapshot| (snapshot.name, snapshot.days));
        (payment, tariff)
    };
    let content = services.bot_content_in(lang::current());
    let what = match tariff {
        Some((name, days)) => ui::text(
            &content,
            "pay.what_tariff",
            &[
                ("tariff", Text(&name)),
                ("term", Text(&crate::text::term(days))),
            ],
        ),
        None => ui::text(&content, "pay.gram_what", &[]),
    };
    Ok(compose(services, &payment, &what))
}

fn refusal(text: &str) -> Value {
    json!({ "error": text })
}

pub async fn on_query(ctx: &BotCtx, query: WidgetQuery) -> Result<(), BotError> {
    let locale = crate::handler::user_locale(&ctx.services, &query.from)
        .await
        .unwrap_or_default();
    lang::scope(locale, answer_query(ctx, query)).await
}

async fn answer_query(ctx: &BotCtx, query: WidgetQuery) -> Result<(), BotError> {
    let answer = match respond(ctx, &query).await {
        Ok(answer) => answer,
        Err(err) => {
            tracing::warn!(error = %err, "запрос виджета оплаты не обработан");
            refusal(lang::pick(
                "Не получилось подготовить перевод. Попробуйте ещё раз.",
                "Couldn't prepare the transfer. Please try again.",
            ))
        }
    };
    ctx.api
        .answer_widget_query(&query.id, &answer.to_string())
        .await?;
    Ok(())
}

async fn owned(
    services: &Services,
    telegram_id: i64,
    payment: Uuid,
) -> Result<Option<Payment>, BotError> {
    let mut conn = services.store().conn().await?;
    let Some(client) = conn
        .clients()
        .find_by_identity(IdentityKind::Telegram, &telegram_id.to_string())
        .await?
    else {
        return Ok(None);
    };
    match conn.payments().by_public_id(payment).await {
        Ok(found) if found.client_id == client.id && invoice(&found).is_some() => Ok(Some(found)),
        Ok(_) | Err(sn_store::StoreError::NotFound(_)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

async fn respond(ctx: &BotCtx, query: &WidgetQuery) -> Result<Value, BotError> {
    let prefix = match query.kind.kind.as_str() {
        "data" => PAY,
        "sent_transaction" => SENT,
        _ => {
            return Ok(refusal(lang::pick(
                "Этот запрос не поддерживается.",
                "This request isn't supported.",
            )));
        }
    };
    let Some(payment) = query
        .data
        .strip_prefix(prefix)
        .and_then(|raw| Uuid::parse_str(raw).ok())
    else {
        return Ok(refusal(lang::pick("Счёт не найден.", "Invoice not found.")));
    };
    let Some(payment) = owned(&ctx.services, query.from.id, payment).await? else {
        return Ok(refusal(lang::pick("Счёт не найден.", "Invoice not found.")));
    };
    if prefix == PAY {
        Ok(prepare(&ctx.services, &payment))
    } else {
        sent(ctx, query, payment).await
    }
}

fn prepare(services: &Services, payment: &Payment) -> Value {
    if payment.status.is_paid_like() {
        return json!({ "done": lang::pick("Этот счёт уже оплачен.", "This invoice is already paid.") });
    }
    let now = services.now();
    if payment.status != PaymentStatus::Pending || payment.expires_at.is_some_and(|at| at <= now) {
        return refusal(lang::pick(
            "Счёт устарел. Оформите оплату заново.",
            "The invoice has expired. Please start the payment again.",
        ));
    }
    if recently_sent(payment.public_id) {
        return refusal(lang::pick(
            "Перевод по этому счёту уже отправлен. Бот напишет, когда он дойдёт.",
            "The transfer for this invoice is already sent. The bot will message you when it arrives.",
        ));
    }
    if !available(services) {
        return refusal(lang::pick(
            "Оплата кошельком Telegram сейчас недоступна.",
            "Paying with Telegram Wallet isn't available right now.",
        ));
    }
    let mut valid_until = now.timestamp() + VALID_SECONDS;
    if let Some(at) = payment.expires_at {
        valid_until = valid_until.min(at.timestamp());
    }
    invoice(payment)
        .and_then(|invoice| invoice.wallet_transaction(valid_until))
        .unwrap_or_else(|| {
            refusal(lang::pick(
                "Кошелёк для оплаты в настройках не подходит.",
                "Telegram Wallet payments aren't set up correctly. Please contact support.",
            ))
        })
}

async fn mark(ctx: &BotCtx, query: &WidgetQuery, payment: &Payment, state: &str) {
    let (Some(chat), Some(message_id)) = (query.chat.as_ref(), query.message_id) else {
        return;
    };
    let Some(invoice) = invoice(payment) else {
        return;
    };
    if let Err(err) = ctx
        .api
        .edit_widget_data(chat.id, message_id, &chat_data(payment, &invoice, state))
        .await
    {
        tracing::debug!(error = %err, "состояние виджета оплаты не обновлено");
    }
}

async fn reload(services: &Services, payment: &Payment) -> Result<Payment, BotError> {
    let mut conn = services.store().conn().await?;
    Ok(conn.payments().get(payment.id).await?)
}

async fn sent(ctx: &BotCtx, query: &WidgetQuery, payment: Payment) -> Result<Value, BotError> {
    let services = &ctx.services;
    remember_sent(payment.public_id);
    tracing::info!(
        payment = %payment.public_id,
        msg_hash = query.kind.msg_hash.as_deref().unwrap_or(""),
        "кошелёк Telegram отправил перевод"
    );
    if payment.status.is_paid_like() {
        mark(ctx, query, &payment, "paid").await;
        return Ok(json!({ "status": "paid" }));
    }
    mark(ctx, query, &payment, "sent").await;
    let started = Instant::now();
    loop {
        let left = CHECK_WINDOW.saturating_sub(started.elapsed());
        match tokio::time::timeout(left, services.inbox().poll_payment(payment.id)).await {
            Ok(Err(err)) => tracing::debug!(error = %err, "проверка перевода GRAM не удалась"),
            Err(_) => return Ok(json!({ "status": "sent" })),
            Ok(Ok(_)) => {}
        }
        let current = reload(services, &payment).await?;
        if current.status.is_paid_like() {
            mark(ctx, query, &current, "paid").await;
            return Ok(json!({ "status": "paid" }));
        }
        if current.status != PaymentStatus::Pending
            || started.elapsed() + CHECK_PAUSE >= CHECK_WINDOW
        {
            return Ok(json!({ "status": "sent" }));
        }
        tokio::time::sleep(CHECK_PAUSE).await;
    }
}
