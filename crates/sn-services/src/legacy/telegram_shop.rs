use super::model::*;
use super::table::{Dump, Row, minor_from_float};
use chrono::{DateTime, Utc};
use sn_domain::money::Currency;
use sn_domain::subscription::SubscriptionKind;
use std::collections::{HashMap, HashSet};

pub const TABLES: &[&str] = &["customer", "purchase", "referral"];

const DAYS_IN_MONTH: u32 = 30;

pub fn looks_like(dump: &Dump) -> bool {
    dump.table("customer")
        .is_some_and(|t| t.has("telegram_id") && t.has("subscription_link"))
}

fn provider_of(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "yookasa" | "yookassa" => "yookassa".into(),
        "crypto" | "cryptobot" => "cryptopay".into(),
        "telegram" | "stars" => "stars".into(),
        p if p == "platega" || p.starts_with("plt_") => "platega".into(),
        "" => "unknown".into(),
        other => other.to_string(),
    }
}

fn external_id(row: &Row<'_>) -> Option<String> {
    ["yookasa_id", "crypto_invoice_id", "platega_id", "wata_id"]
        .into_iter()
        .find_map(|column| row.string(column))
}

fn paid_customers(dump: &Dump) -> HashSet<String> {
    dump.rows("purchase")
        .filter(|r| {
            r.text("status")
                .is_some_and(|s| s.eq_ignore_ascii_case("paid"))
                && r.f64("amount").is_some_and(|a| a > 0.0)
        })
        .filter_map(|r| r.string("customer_id"))
        .collect()
}

fn clients(dump: &Dump, legacy: &mut Legacy) -> HashMap<i64, String> {
    let mut by_tg: HashMap<i64, String> = HashMap::new();
    for row in dump.rows("customer") {
        let Some(old_id) = row.string("id") else {
            continue;
        };
        let telegram_id = row.i64("telegram_id").filter(|id| *id > 0);
        if let Some(tg) = telegram_id {
            if by_tg.contains_key(&tg) {
                legacy.problem(
                    "duplicate_telegram",
                    "client",
                    Some(old_id),
                    Some(format!("Telegram {tg}")),
                    "Этот Telegram ID уже есть у другого клиента в выгрузке — клиент пропущен",
                );
                continue;
            }
            by_tg.insert(tg, old_id.clone());
        }
        legacy.clients.push(LClient {
            telegram_id,
            username: None,
            first_name: None,
            last_name: None,
            email: None,
            email_raw: None,
            email_verified: false,
            password_hash: None,
            google_id: None,
            apple_id: None,
            locale: locale_of(row.text("language")),
            blocked: false,
            block_reason: None,
            bot_unreachable: false,
            balance_minor: 0,
            balance_mixed: false,
            referral_code: telegram_id.map(|tg| tg.to_string()),
            referrer_old_id: None,
            referral_percent: None,
            utm: None,
            trial_used: row.text("subscription_link").is_some() || row.text("expire_at").is_some(),
            created_at: row.time("created_at"),
            old_id,
        });
    }
    by_tg
}

fn referrals(dump: &Dump, legacy: &mut Legacy, by_tg: &HashMap<i64, String>) {
    let mut lost = 0u64;
    let mut referrer_of: HashMap<String, String> = HashMap::new();
    for row in dump.rows("referral") {
        let (Some(referrer), Some(referee)) = (row.i64("referrer_id"), row.i64("referee_id"))
        else {
            continue;
        };
        match (by_tg.get(&referrer), by_tg.get(&referee)) {
            (Some(referrer), Some(referee)) if referrer != referee => {
                referrer_of
                    .entry(referee.clone())
                    .or_insert_with(|| referrer.clone());
            }
            _ => lost += 1,
        }
    }
    for client in &mut legacy.clients {
        client.referrer_old_id = referrer_of.remove(&client.old_id);
    }
    legacy.skip(
        "referrals_lost",
        lost,
        "приглашений ссылаются на клиентов, которых нет в копии: связь не переносится.",
    );
}

fn subscriptions(dump: &Dump, legacy: &mut Legacy, paid: &HashSet<String>) {
    let known: HashMap<String, (Option<i64>, Option<DateTime<Utc>>)> = legacy
        .clients
        .iter()
        .map(|c| (c.old_id.clone(), (c.telegram_id, c.created_at)))
        .collect();
    for row in dump.rows("customer") {
        let Some(old_id) = row.string("id") else {
            continue;
        };
        let Some((telegram_id, created_at)) = known.get(&old_id).copied() else {
            continue;
        };
        let link = row.text("subscription_link");
        let expire_at = row.time("expire_at");
        if link.is_none() && expire_at.is_none() {
            continue;
        }
        let panel_name = match telegram_id {
            Some(tg) => format!("{old_id}_{tg}"),
            None => format!("customer:{old_id}"),
        };
        legacy.subscriptions.push(LSubscription {
            old_id: panel_name,
            client_old_id: old_id.clone(),
            tariff_old_id: None,
            kind: if paid.contains(&old_id) {
                SubscriptionKind::Paid
            } else {
                SubscriptionKind::Trial
            },
            remna_id: None,
            remna_uuid: None,
            short_uuid: short_uuid_from_url(link),
            expire_at,
            extra_devices: 0,
            autorenew: false,
            autorenew_days: None,
            autorenew_period_old_id: None,
            ordinal: 0,
            gifted: false,
            created_at,
        });
    }
    let count = legacy.subscriptions.len() as u64;
    legacy.skip(
        "tariffs_env",
        count,
        "подписок перейдут без тарифа: цены и сроки этот бот берёт из своего файла .env (PRICE_1, PRICE_3, PRICE_6, PRICE_12), а не из базы. Заведите тарифы вручную в «Тарифах» — на 30, 90, 180 и 360 дней; при продлении клиент выберет тариф.",
    );
}

fn payments(dump: &Dump, legacy: &mut Legacy) {
    let mut unpaid = 0u64;
    let mut free = 0u64;
    for row in dump.rows("purchase") {
        let (Some(old_id), Some(client)) = (row.string("id"), row.string("customer_id")) else {
            continue;
        };
        if !row
            .text("status")
            .is_some_and(|s| s.eq_ignore_ascii_case("paid"))
        {
            unpaid += 1;
            continue;
        }
        let stars = row
            .text("currency")
            .is_some_and(|c| c.eq_ignore_ascii_case("STARS") || c.eq_ignore_ascii_case("XTR"))
            || row
                .text("invoice_type")
                .is_some_and(|t| t.eq_ignore_ascii_case("telegram"));
        let amount = match row.f64("amount") {
            Some(value) if stars => value.round() as i64,
            Some(value) => minor_from_float(value).unwrap_or(0),
            None => 0,
        };
        if amount <= 0 {
            free += 1;
            continue;
        }
        let months = row
            .i64("month")
            .and_then(|m| u32::try_from(m).ok())
            .filter(|m| *m > 0);
        legacy.payments.push(LPayment {
            client_old_id: client,
            kind: PaymentKind::Subscription,
            refunded: false,
            amount_minor: amount,
            currency: if stars {
                PayCurrencyLegacy::Stars
            } else {
                PayCurrencyLegacy::Money(Currency::Rub)
            },
            provider: provider_of(row.text("invoice_type").unwrap_or("")),
            external_id: external_id(&row),
            tariff_old_id: None,
            days: months.map(|m| m * DAYS_IN_MONTH),
            subscription_old_id: None,
            label: months.map(|m| format!("Подписка на {m} мес.")),
            created_at: row.time("created_at"),
            paid_at: row.time("paid_at").or_else(|| row.time("created_at")),
            old_id,
        });
    }
    legacy.skip(
        "payments_unpaid",
        unpaid,
        "неоплаченных и отменённых счетов: в истории они не нужны.",
    );
    legacy.skip(
        "payments_free",
        free,
        "покупок на ноль рублей: в истории платежей их нет, подписки переносятся.",
    );
}

pub fn map(dump: &Dump) -> Legacy {
    let mut legacy = Legacy::new(Source::TelegramShop, Currency::Rub);
    legacy.dumped_at = dump.dumped_at;
    let paid = paid_customers(dump);
    let by_tg = clients(dump, &mut legacy);
    referrals(dump, &mut legacy, &by_tg);
    subscriptions(dump, &mut legacy, &paid);
    payments(dump, &mut legacy);
    legacy
}
