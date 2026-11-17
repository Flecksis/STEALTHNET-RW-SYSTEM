use super::model::*;
use super::table::{Dump, Row, minor_from_float};
use sn_domain::money::Currency;
use sn_domain::subscription::SubscriptionKind;
use sn_domain::traffic::TrafficReset;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub const TABLES: &[&str] = &[
    "clients",
    "tariff_categories",
    "tariffs",
    "tariff_price_options",
    "subscriptions",
    "payments",
    "promo_codes",
    "promo_code_usages",
    "promo_groups",
    "tickets",
    "ticket_messages",
    "system_settings",
    "client_trial_usages",
    "withdrawal_requests",
    "proxy_slots",
    "singbox_slots",
    "gift_codes",
];

pub fn looks_like(dump: &Dump) -> bool {
    dump.table("clients")
        .is_some_and(|t| t.has("telegram_id") && t.has("balance"))
        || dump.table("tariff_price_options").is_some()
}

fn currency_of(raw: Option<&str>) -> Option<Currency> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "usd" | "$" => Some(Currency::Usd),
        "rub" | "rur" | "₽" => Some(Currency::Rub),
        _ => None,
    }
}

fn settings(dump: &Dump) -> HashMap<String, String> {
    dump.rows("system_settings")
        .filter_map(|r| Some((r.string("key")?, r.raw("value").unwrap_or("").to_string())))
        .collect()
}

fn provider_of(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "telegram_stars" | "stars" => "stars".into(),
        "cryptobot" | "crypto_pay" | "cryptopay" => "cryptopay".into(),
        "yoomoney_form" => "yoomoney".into(),
        "" => "unknown".into(),
        other => other.to_string(),
    }
}

fn paid_status(row: &Row<'_>) -> Option<bool> {
    match row.text("status")?.to_ascii_uppercase().as_str() {
        "PAID" => Some(false),
        "REFUNDED" => Some(true),
        _ => None,
    }
}

fn decide_currency(dump: &Dump, settings: &HashMap<String, String>) -> (Currency, HashSet<String>) {
    let mut sums: HashMap<Currency, i64> = HashMap::new();
    let mut per_client: HashMap<String, HashSet<Currency>> = HashMap::new();
    for row in dump.rows("payments") {
        if paid_status(&row).is_none() {
            continue;
        }
        let provider = row.text("provider").map(provider_of).unwrap_or_default();
        if provider == "stars" || provider == "admin_grant" {
            continue;
        }
        let Some(currency) = currency_of(row.text("currency")) else {
            continue;
        };
        let amount = row.f64("amount").and_then(minor_from_float).unwrap_or(0);
        *sums.entry(currency).or_default() += amount;
        if provider != "balance"
            && let Some(client) = row.string("client_id")
        {
            per_client.entry(client).or_default().insert(currency);
        }
    }
    let setting = settings
        .get("default_currency")
        .and_then(|v| currency_of(Some(v.trim_matches(['"', '[', ']']))));
    let currency = match sums.len() {
        1 => *sums.keys().next().unwrap_or(&Currency::Usd),
        0 => setting.unwrap_or_else(|| {
            let mut tally: HashMap<Currency, usize> = HashMap::new();
            for row in dump.rows("tariffs") {
                if let Some(c) = currency_of(row.text("currency")) {
                    *tally.entry(c).or_default() += 1;
                }
            }
            tally
                .into_iter()
                .max_by_key(|(_, n)| *n)
                .map_or(Currency::Usd, |(c, _)| c)
        }),
        _ => setting.unwrap_or_else(|| {
            sums.iter()
                .max_by_key(|(_, sum)| **sum)
                .map_or(Currency::Usd, |(c, _)| *c)
        }),
    };
    let mixed = per_client
        .into_iter()
        .filter(|(_, set)| set.len() > 1)
        .map(|(client, _)| client)
        .collect();
    (currency, mixed)
}

fn percent(value: Option<f64>) -> Option<i32> {
    value
        .filter(|v| (0.0..=100.0).contains(v))
        .map(|v| v.round() as i32)
}

fn clients(dump: &Dump, legacy: &mut Legacy, mixed: &HashSet<String>) {
    let trial_usage: HashSet<String> = dump
        .rows("client_trial_usages")
        .filter_map(|r| r.string("client_id"))
        .collect();
    let mut seen_tg = HashSet::new();
    for row in dump.rows("clients") {
        let Some(old_id) = row.string("id") else {
            continue;
        };
        if row
            .text("role")
            .is_some_and(|r| !r.eq_ignore_ascii_case("CLIENT"))
        {
            continue;
        }
        let telegram_id = row.i64("telegram_id").filter(|id| *id > 0);
        if let Some(tg) = telegram_id
            && !seen_tg.insert(tg)
        {
            legacy.problem(
                "duplicate_telegram",
                "client",
                Some(old_id.clone()),
                Some(format!("Telegram {tg}")),
                "Этот Telegram ID уже есть у другого клиента в выгрузке — клиент пропущен",
            );
            continue;
        }
        let balance = row
            .f64("balance")
            .and_then(minor_from_float)
            .unwrap_or(0)
            .max(0);
        let is_mixed = mixed.contains(&old_id) && balance > 0;
        let utm = Utm {
            source: row.string("utm_source"),
            medium: row.string("utm_medium"),
            campaign: row.string("utm_campaign"),
            content: row.string("utm_content"),
            term: row.string("utm_term"),
        };
        let has_utm = utm.source.is_some()
            || utm.medium.is_some()
            || utm.campaign.is_some()
            || utm.content.is_some()
            || utm.term.is_some();
        legacy.clients.push(LClient {
            telegram_id,
            username: clean_username(row.text("telegram_username")),
            first_name: None,
            last_name: None,
            email: clean_email(row.text("email")),
            email_verified: true,
            password_hash: password_hash(row.text("password_hash")),
            google_id: row.string("google_id"),
            apple_id: row.string("apple_id"),
            locale: locale_of(row.text("preferred_lang")),
            blocked: row.flag("is_blocked"),
            block_reason: row.string("block_reason"),
            bot_unreachable: row.flag("telegram_unreachable"),
            balance_minor: if is_mixed { 0 } else { balance },
            balance_mixed: is_mixed,
            referral_code: row.string("referral_code"),
            referrer_old_id: row.string("referrer_id"),
            referral_percent: percent(row.f64("referral_percent")),
            utm: has_utm.then_some(utm),
            trial_used: row.flag("trial_used") || trial_usage.contains(&old_id),
            created_at: row.time("created_at"),
            old_id,
        });
        if is_mixed {
            let last = legacy.clients.last().map(LClient::who);
            legacy.problem(
                "balance_mixed",
                "client",
                legacy.clients.last().map(|c| c.old_id.clone()),
                last,
                format!(
                    "Баланс {} копился и в рублях, и в долларах — не перенесён, решите вручную",
                    format_minor(balance)
                ),
            );
        }
    }
    let mixed_count = legacy.clients.iter().filter(|c| c.balance_mixed).count() as u64;
    legacy.skip(
        "balance_mixed",
        mixed_count,
        "клиентов пополняли баланс и в рублях, и в долларах. Такой баланс не переносим — суммы в отчёте, решите вручную.",
    );
}

pub fn format_minor(minor: i64) -> String {
    format!("{}.{:02}", minor / 100, (minor % 100).abs())
}

fn uuids(values: Vec<String>) -> Vec<Uuid> {
    values
        .iter()
        .filter_map(|v| Uuid::parse_str(v.trim()).ok())
        .collect()
}

fn tariffs(dump: &Dump, legacy: &mut Legacy) -> HashMap<String, u32> {
    for row in dump.rows("tariff_categories") {
        let (Some(old_id), Some(name)) = (row.string("id"), row.string("name")) else {
            continue;
        };
        legacy.categories.push(LCategory {
            old_id,
            name,
            sort: row.i64("sort_order").unwrap_or(0) as i32,
        });
    }
    let mut option_days = HashMap::new();
    let mut options: HashMap<String, Vec<LPeriod>> = HashMap::new();
    for row in dump.rows("tariff_price_options") {
        let (Some(old_id), Some(tariff), Some(days)) = (
            row.string("id"),
            row.string("tariff_id"),
            row.i64("duration_days"),
        ) else {
            continue;
        };
        let Ok(days) = u32::try_from(days) else {
            continue;
        };
        if days == 0 {
            continue;
        }
        option_days.insert(old_id.clone(), days);
        options.entry(tariff).or_default().push(LPeriod {
            old_id: Some(old_id),
            days,
            price_minor: row.f64("price").and_then(minor_from_float),
            sort: row.i64("sort_order").unwrap_or(0) as i32,
        });
    }
    for row in dump.rows("tariffs") {
        let (Some(old_id), Some(category), Some(name)) = (
            row.string("id"),
            row.string("category_id"),
            row.string("name"),
        ) else {
            continue;
        };
        let currency = currency_of(row.text("currency")).unwrap_or(legacy.currency);
        let mut periods = options.remove(&old_id).unwrap_or_default();
        if periods.is_empty()
            && let Some(days) = row.i64("duration_days").and_then(|d| u32::try_from(d).ok())
            && days > 0
        {
            periods.push(LPeriod {
                old_id: None,
                days,
                price_minor: row.f64("price").and_then(minor_from_float),
                sort: 0,
            });
        }
        let mut seen = HashSet::new();
        periods.retain(|p| seen.insert(p.days));
        let raw_reset = row.text("traffic_reset_mode");
        let reset = parse_reset(raw_reset).unwrap_or(TrafficReset::NoReset);
        let mut notes = Vec::new();
        if raw_reset.is_some_and(|r| r.eq_ignore_ascii_case("carry_over")) {
            notes.push(
                "перенос остатка трафика в новой системе нет — стоит «Без сброса»".to_string(),
            );
        }
        if currency != legacy.currency {
            notes.push(format!(
                "цены тарифа в {}, а основная валюта {} — тариф снят с продажи",
                currency.code(),
                legacy.currency.code()
            ));
        }
        let extra_max = row
            .i64("max_extra_devices")
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0);
        let extra_price = row
            .f64("price_per_extra_device")
            .and_then(minor_from_float)
            .filter(|v| *v > 0);
        legacy.tariffs.push(LTariff {
            category_old_id: category,
            name,
            description: row.string("description"),
            sort: row.i64("sort_order").unwrap_or(0) as i32,
            on_sale: currency == legacy.currency,
            currency,
            periods,
            devices_included: row
                .i64("included_devices")
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(1),
            extra_devices_max: if extra_price.is_some() { extra_max } else { 0 },
            extra_device_monthly_minor: extra_price.filter(|_| extra_max > 0),
            traffic_limit_bytes: row
                .i64("traffic_limit_bytes")
                .and_then(|v| u64::try_from(v).ok())
                .filter(|v| *v > 0),
            traffic_reset: reset,
            squads: uuids(row.list("internal_squad_uuids")),
            external_squad: None,
            note: (!notes.is_empty()).then(|| notes.join("; ")),
            old_id,
        });
    }
    option_days
}

fn subscriptions(dump: &Dump, legacy: &mut Legacy) {
    let mut gifts = 0u64;
    let mut with_sub: HashSet<String> = HashSet::new();
    for row in dump.rows("subscriptions") {
        let (Some(old_id), Some(owner)) = (row.string("id"), row.string("owner_id")) else {
            continue;
        };
        if row.text("gift_status").is_some_and(|s| {
            matches!(
                s.to_ascii_uppercase().as_str(),
                "GIFT_RESERVED" | "GIFT_CODE_ACTIVE"
            )
        }) {
            gifts += 1;
            continue;
        }
        with_sub.insert(owner.clone());
        let raw_remna = row.string("remnawave_uuid");
        let remna_id = raw_remna.as_deref().and_then(|v| v.parse::<i64>().ok());
        let kind = if row.text("trial_id").is_some() {
            SubscriptionKind::Trial
        } else if row.flag("promo_granted") {
            SubscriptionKind::Promo
        } else if row.flag("purchased_as_gift") {
            SubscriptionKind::Gift
        } else if row.text("tariff_id").is_none() && row.text("custom_price").is_some() {
            SubscriptionKind::Custom
        } else {
            SubscriptionKind::Paid
        };
        legacy.subscriptions.push(LSubscription {
            client_old_id: owner,
            tariff_old_id: row.string("tariff_id"),
            kind,
            remna_id,
            remna_uuid: raw_remna.filter(|_| remna_id.is_none()),
            short_uuid: row.string("short_uuid"),
            expire_at: row.time("expire_at"),
            extra_devices: row
                .i64("extra_devices")
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0),
            autorenew: row.flag("auto_renew_enabled"),
            autorenew_days: None,
            autorenew_period_old_id: row.string("auto_renew_price_option_id"),
            ordinal: row
                .i64("subscription_index")
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0),
            created_at: row.time("created_at"),
            old_id,
        });
    }
    for row in dump.rows("clients") {
        let Some(id) = row.string("id") else { continue };
        if with_sub.contains(&id) {
            continue;
        }
        let Some(raw) = row.string("remnawave_uuid") else {
            continue;
        };
        let remna_id = raw.parse::<i64>().ok();
        legacy.subscriptions.push(LSubscription {
            old_id: format!("client:{id}"),
            client_old_id: id,
            tariff_old_id: row.string("current_tariff_id"),
            kind: SubscriptionKind::Paid,
            remna_id,
            remna_uuid: (remna_id.is_none()).then_some(raw),
            short_uuid: None,
            expire_at: None,
            extra_devices: 0,
            autorenew: row.flag("auto_renew_enabled"),
            autorenew_days: None,
            autorenew_period_old_id: row.string("auto_renew_price_option_id"),
            ordinal: 0,
            created_at: row.time("created_at"),
        });
    }
    legacy.skip(
        "gift_unclaimed",
        gifts,
        "подарочных подписок ещё не активировали. Их нет у получателей — после переключения выдайте вручную.",
    );
}

fn payments(dump: &Dump, legacy: &mut Legacy, option_days: &HashMap<String, u32>) {
    let mut unpaid = 0u64;
    let mut free = 0u64;
    let mut unknown_currency = 0u64;
    for row in dump.rows("payments") {
        let (Some(old_id), Some(client)) = (row.string("id"), row.string("client_id")) else {
            continue;
        };
        let Some(refunded) = paid_status(&row) else {
            unpaid += 1;
            continue;
        };
        let provider = row
            .text("provider")
            .map(provider_of)
            .unwrap_or_else(|| "unknown".into());
        let meta = row.json("metadata").unwrap_or(serde_json::Value::Null);
        let stars = meta
            .pointer("/stars/total")
            .and_then(serde_json::Value::as_i64)
            .filter(|n| *n > 0);
        let currency = if provider == "stars" && stars.is_some() {
            PayCurrencyLegacy::Stars
        } else if let Some(c) = currency_of(row.text("currency")) {
            PayCurrencyLegacy::Money(c)
        } else {
            unknown_currency += 1;
            continue;
        };
        let amount = match currency {
            PayCurrencyLegacy::Stars => stars.unwrap_or(0),
            PayCurrencyLegacy::Money(_) => {
                row.f64("amount").and_then(minor_from_float).unwrap_or(0)
            }
        };
        if amount <= 0 || provider == "admin_grant" {
            free += 1;
            continue;
        }
        let (kind, label) =
            if row.text("tariff_id").is_some() || row.text("tariff_price_option_id").is_some() {
                (PaymentKind::Subscription, None)
            } else if row.text("proxy_tariff_id").is_some() {
                (PaymentKind::Other, Some("Прокси".to_string()))
            } else if row.text("singbox_tariff_id").is_some() {
                (PaymentKind::Other, Some("Sing-box".to_string()))
            } else if meta.get("extraOption").is_some() {
                (PaymentKind::Other, Some("Доп. опция".to_string()))
            } else if meta.get("customBuild").is_some() {
                (PaymentKind::Other, Some("Своя подписка".to_string()))
            } else if provider == "balance" {
                (PaymentKind::Other, Some("Покупка с баланса".to_string()))
            } else {
                (PaymentKind::TopUp, None)
            };
        let days = row
            .text("tariff_price_option_id")
            .and_then(|id| option_days.get(id).copied())
            .or_else(|| {
                meta.pointer("/customBuild/durationDays")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|d| u32::try_from(d).ok())
            });
        legacy.payments.push(LPayment {
            client_old_id: client,
            kind,
            refunded,
            amount_minor: amount,
            currency,
            provider,
            external_id: row.string("external_id"),
            tariff_old_id: row.string("tariff_id"),
            days,
            subscription_old_id: row.string("subscription_id"),
            label,
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
        "выдач без оплаты: в истории платежей их нет, подписки переносятся.",
    );
    legacy.skip(
        "payments_currency",
        unknown_currency,
        "оплат в незнакомой валюте: их нет в истории.",
    );
}

fn promo_code(raw: &str) -> Option<String> {
    let code = raw.trim().to_ascii_uppercase();
    let valid = (3..=32).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    valid.then_some(code)
}

fn promos(dump: &Dump, legacy: &mut Legacy) {
    let mut bad = 0u64;
    for row in dump.rows("promo_codes") {
        let (Some(old_id), Some(raw)) = (row.string("id"), row.string("code")) else {
            continue;
        };
        let Some(code) = promo_code(&raw) else {
            bad += 1;
            legacy.problem(
                "promo_code_format",
                "promo",
                Some(old_id),
                Some(raw),
                "Код промокода должен быть из латинских букв и цифр, от 3 до 32 знаков — не перенесён",
            );
            continue;
        };
        let kind = row.text("type").unwrap_or("").to_ascii_uppercase();
        let value = if kind == "FREE_DAYS" {
            row.i64("duration_days")
                .and_then(|d| u32::try_from(d).ok())
                .filter(|d| (1..=3650).contains(d))
                .map(PromoValue::Days)
        } else if let Some(p) = row.f64("discount_percent").filter(|p| *p > 0.0) {
            Some(PromoValue::Percent(p.round().clamp(1.0, 100.0) as u8))
        } else {
            row.f64("discount_fixed")
                .and_then(minor_from_float)
                .filter(|v| *v > 0)
                .map(PromoValue::Fixed)
        };
        let Some(value) = value else {
            bad += 1;
            legacy.problem(
                "promo_value",
                "promo",
                Some(old_id),
                Some(code),
                "У промокода нет скидки или дней — не перенесён",
            );
            continue;
        };
        legacy.promos.push(LPromo {
            code,
            value,
            max_uses: row
                .i64("max_uses")
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v > 0),
            per_client: row
                .i64("max_uses_per_client")
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v > 0)
                .unwrap_or(1),
            valid_from: None,
            valid_until: row.time("expires_at"),
            active: row.bool("is_active").unwrap_or(true),
            first_purchase_only: false,
            tariff_old_id: row.string("tariff_id"),
            note: row.string("name"),
            old_id,
        });
    }
    for row in dump.rows("promo_code_usages") {
        let (Some(old_id), Some(promo), Some(client)) = (
            row.string("id"),
            row.string("promo_code_id"),
            row.string("client_id"),
        ) else {
            continue;
        };
        legacy.promo_uses.push(LPromoUse {
            old_id,
            promo_old_id: promo,
            client_old_id: client,
            used_at: row.time("created_at"),
        });
    }
    legacy.skip(
        "promo_invalid",
        bad,
        "промокодов с кодом или скидкой, которые новая система не принимает: список в отчёте.",
    );
    legacy.skip(
        "promo_links",
        dump.count("promo_groups") as u64,
        "промо-ссылок на бесплатные дни: в новой системе их заменяют промокоды на дни.",
    );
}

fn tickets(dump: &Dump, legacy: &mut Legacy) {
    let mut messages: HashMap<String, Vec<LMessage>> = HashMap::new();
    for row in dump.rows("ticket_messages") {
        let (Some(old_id), Some(ticket)) = (row.string("id"), row.string("ticket_id")) else {
            continue;
        };
        let body = row.raw("content").unwrap_or("").trim().to_string();
        let attachments = row
            .json("attachments")
            .and_then(|v| v.as_array().map(Vec::len))
            .unwrap_or(0);
        let body = match (body.is_empty(), attachments) {
            (true, 0) => continue,
            (true, n) => format!("[вложений: {n}, файлы остались в старой панели]"),
            (false, 0) => body,
            (false, n) => format!("{body}\n[вложений: {n}, файлы остались в старой панели]"),
        };
        messages.entry(ticket).or_default().push(LMessage {
            old_id,
            from_support: row
                .text("author_type")
                .is_some_and(|a| a.eq_ignore_ascii_case("support")),
            body,
            read: row.flag("is_read"),
            created_at: row.time("created_at"),
        });
    }
    for row in dump.rows("tickets") {
        let (Some(old_id), Some(client)) = (row.string("id"), row.string("client_id")) else {
            continue;
        };
        let mut list = messages.remove(&old_id).unwrap_or_default();
        list.sort_by_key(|m| m.created_at);
        legacy.tickets.push(LTicket {
            client_old_id: client,
            subject: row.string("subject"),
            open: !row
                .text("status")
                .is_some_and(|s| s.eq_ignore_ascii_case("closed")),
            created_at: row.time("created_at"),
            closed_at: row.time("closed_at"),
            messages: list,
            old_id,
        });
    }
}

fn leftovers(dump: &Dump, legacy: &mut Legacy) {
    let slots = (dump.count("proxy_slots") + dump.count("singbox_slots")) as u64;
    legacy.skip(
        "products_absent",
        slots,
        "прокси и sing-box: в новой системе этих товаров нет. Оплаты за них останутся в истории.",
    );
    let pending: Vec<(String, i64)> = dump
        .rows("withdrawal_requests")
        .filter(|r| {
            r.text("status")
                .is_some_and(|s| s.eq_ignore_ascii_case("PENDING"))
        })
        .filter_map(|r| {
            Some((
                r.string("client_id")?,
                r.f64("amount").and_then(minor_from_float).unwrap_or(0),
            ))
        })
        .collect();
    for (client, amount) in &pending {
        legacy.problem(
            "withdrawal_pending",
            "client",
            Some(client.clone()),
            None,
            format!(
                "Заявка на вывод {} не решена в старой панели",
                format_minor(*amount)
            ),
        );
    }
    legacy.skip(
        "withdrawal_pending",
        pending.len() as u64,
        "заявок на вывод реферальных денег ждут решения. Решите их в старой панели до переключения.",
    );
    let gifts = dump
        .rows("gift_codes")
        .filter(|r| {
            r.text("status")
                .is_some_and(|s| s.eq_ignore_ascii_case("ACTIVE"))
        })
        .count() as u64;
    legacy.skip(
        "gift_codes",
        gifts,
        "подарочных кодов не активировали. Подарков в новой системе пока нет.",
    );
}

pub fn map(dump: &Dump) -> Legacy {
    let settings = settings(dump);
    let (currency, mixed) = decide_currency(dump, &settings);
    let mut legacy = Legacy::new(Source::Stealthnet, currency);
    legacy.dumped_at = dump.dumped_at;
    let option_days = tariffs(dump, &mut legacy);
    clients(dump, &mut legacy, &mixed);
    subscriptions(dump, &mut legacy);
    payments(dump, &mut legacy, &option_days);
    promos(dump, &mut legacy);
    tickets(dump, &mut legacy);
    leftovers(dump, &mut legacy);
    legacy
}
