use super::model::*;
use super::table::{Dump, Row, minor_from_float};
use chrono::Duration;
use sn_domain::money::Currency;
use sn_domain::subscription::SubscriptionKind;
use sn_domain::traffic::TrafficReset;
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

pub const TABLES: &[&str] = &[
    "users",
    "subscriptions",
    "plans",
    "plan_durations",
    "plan_prices",
    "transactions",
    "promocodes",
    "promocode_activations",
    "referrals",
    "settings",
    "payment_gateways",
    "user_oauth_providers",
    "ad_links",
];

pub const MAIN_CATEGORY: &str = "remnashop:main";
pub const MAX_DEVICES: u32 = 100;

pub fn looks_like(dump: &Dump) -> bool {
    dump.table("plan_durations").is_some()
        || dump
            .table("subscriptions")
            .is_some_and(|t| t.has("user_remna_id"))
        || dump
            .table("users")
            .is_some_and(|t| t.has("personal_discount") && t.has("purchase_discount"))
}

struct Users {
    by_tg: HashMap<i64, String>,
}

impl Users {
    fn new(dump: &Dump) -> Self {
        Users {
            by_tg: dump
                .rows("users")
                .filter_map(|r| Some((r.i64("telegram_id")?, r.string("id")?)))
                .collect(),
        }
    }

    fn of(&self, row: &Row<'_>, id: &str, tg: &str) -> Option<String> {
        row.string(id)
            .or_else(|| row.i64(tg).and_then(|t| self.by_tg.get(&t).cloned()))
    }
}

fn currency_of(raw: Option<&str>) -> Option<Currency> {
    match raw?.trim().to_ascii_uppercase().as_str() {
        "RUB" => Some(Currency::Rub),
        "USD" => Some(Currency::Usd),
        _ => None,
    }
}

fn decide_currency(dump: &Dump) -> Currency {
    if let Some(found) = dump
        .rows("settings")
        .find_map(|r| currency_of(r.text("default_currency")))
    {
        return found;
    }
    let mut tally: HashMap<Currency, usize> = HashMap::new();
    for row in dump.rows("transactions") {
        if row
            .text("status")
            .is_some_and(|s| s.eq_ignore_ascii_case("COMPLETED"))
            && let Some(c) = currency_of(row.text("currency"))
        {
            *tally.entry(c).or_default() += 1;
        }
    }
    if tally.is_empty() {
        for row in dump.rows("plan_prices") {
            if let Some(c) = currency_of(row.text("currency")) {
                *tally.entry(c).or_default() += 1;
            }
        }
    }
    tally
        .into_iter()
        .max_by_key(|(c, n)| (*n, *c == Currency::Rub))
        .map_or(Currency::Rub, |(c, _)| c)
}

fn number(value: Option<&serde_json::Value>) -> Option<f64> {
    let value = value?;
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
        .filter(|v| v.is_finite())
}

fn snapshot(row: &Row<'_>) -> serde_json::Value {
    row.json("plan_snapshot")
        .or_else(|| row.json("plan"))
        .unwrap_or(serde_json::Value::Null)
}

fn snapshot_id(plan: &serde_json::Value) -> Option<String> {
    match plan.get("id")? {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

fn snapshot_days(plan: &serde_json::Value) -> Option<u32> {
    number(plan.get("duration"))
        .filter(|d| *d >= 1.0 && *d <= 36_500.0)
        .map(|d| d as u32)
}

fn snapshot_name(plan: &serde_json::Value) -> Option<String> {
    plan.get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

fn uuids(values: Vec<String>) -> Vec<Uuid> {
    let mut out: Vec<Uuid> = values
        .iter()
        .filter_map(|v| Uuid::parse_str(v.trim()).ok())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn external_squad(row: &Row<'_>) -> Option<Uuid> {
    let raw = row.text("external_squad")?;
    if raw.starts_with('{') || raw.starts_with('[') {
        return uuids(row.list("external_squad")).into_iter().next();
    }
    Uuid::parse_str(raw).ok()
}

fn provider_of(raw: Option<&str>) -> String {
    match raw.map(|r| r.trim().to_ascii_lowercase()).as_deref() {
        Some("telegram_stars") => "stars".into(),
        Some("") | None => "unknown".into(),
        Some(other) => other.to_string(),
    }
}

fn gateway_title(raw: &str) -> String {
    match raw.trim().to_ascii_uppercase().as_str() {
        "TELEGRAM_STARS" => "Telegram Stars".into(),
        "YOOKASSA" => "ЮKassa".into(),
        "YOOMONEY" => "ЮMoney".into(),
        "CRYPTOPAY" => "CryptoPay".into(),
        "HELEKET" => "Heleket".into(),
        "PLATEGA" => "Platega".into(),
        "CRYPTOMUS" => "Cryptomus".into(),
        "FREEKASSA" => "FreeKassa".into(),
        "ROBOKASSA" => "Robokassa".into(),
        other => {
            let lower = other.to_ascii_lowercase();
            let mut chars = lower.chars();
            chars
                .next()
                .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        }
    }
}

fn promo_code(raw: &str) -> Option<String> {
    let code = raw.trim().to_ascii_uppercase();
    let valid = (3..=32).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    valid.then_some(code)
}

fn days_word(days: &[i64]) -> String {
    days.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn tariffs(dump: &Dump, legacy: &mut Legacy) -> HashMap<String, u32> {
    let mut prices: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    for row in dump.rows("plan_prices") {
        let (Some(duration), Some(currency), Some(price)) = (
            row.string("plan_duration_id"),
            row.text("currency"),
            row.f64("price"),
        ) else {
            continue;
        };
        prices
            .entry(duration)
            .or_default()
            .push((currency.to_ascii_uppercase(), price));
    }
    let mut durations: HashMap<String, Vec<(String, i64, i64)>> = HashMap::new();
    for row in dump.rows("plan_durations") {
        let (Some(id), Some(plan)) = (row.string("id"), row.string("plan_id")) else {
            continue;
        };
        durations.entry(plan).or_default().push((
            id,
            row.i64("days").unwrap_or(0),
            row.i64("order_index").unwrap_or(0),
        ));
    }
    let code = legacy.currency.code();
    let mut devices = HashMap::new();
    for row in dump.rows("plans") {
        let (Some(old_id), Some(name)) = (row.string("id"), row.string("name")) else {
            continue;
        };
        let mut list = durations.remove(&old_id).unwrap_or_default();
        list.sort_by_key(|(id, days, order)| (*order, *days, id.clone()));
        let mut notes = Vec::new();
        let mut periods: Vec<LPeriod> = Vec::new();
        let mut endless = false;
        let mut stars = false;
        let mut missing = Vec::new();
        for (id, days, order) in &list {
            let offers = prices.get(id).map(Vec::as_slice).unwrap_or_default();
            stars |= offers.iter().any(|(c, p)| c == "XTR" && *p > 0.0);
            let Some(days) = u32::try_from(*days)
                .ok()
                .filter(|d| (1..=36_500).contains(d))
            else {
                endless = true;
                continue;
            };
            if periods.iter().any(|p| p.days == days) {
                continue;
            }
            let price = offers
                .iter()
                .find(|(c, _)| c == code)
                .and_then(|(_, p)| minor_from_float(*p))
                .filter(|p| *p >= 0);
            if price.is_none() {
                missing.push(i64::from(days));
            }
            periods.push(LPeriod {
                old_id: Some(id.clone()),
                days,
                price_minor: price,
                sort: *order as i32,
            });
        }
        if endless {
            notes.push("бессрочного срока в новой системе нет — он не перенесён".to_string());
        }
        if !missing.is_empty() {
            notes.push(format!(
                "нет цены в {code} для сроков {} дн. — задайте её сами",
                days_word(&missing)
            ));
        }
        if stars {
            notes.push("цены в звёздах задайте сами".to_string());
        }
        let trial = row.flag("is_trial");
        if trial {
            notes.push(
                "это пробный тариф старого бота: пробный период здесь настраивается отдельно, тариф снят с продажи".to_string(),
            );
        }
        let availability = row
            .text("availability")
            .unwrap_or("ALL")
            .to_ascii_uppercase();
        let private = matches!(availability.as_str(), "ALLOWED" | "LINK");
        match availability.as_str() {
            "ALLOWED" | "LINK" => notes.push(
                "в старом боте тариф видели только избранные клиенты — тариф снят с продажи"
                    .to_string(),
            ),
            "NEW" => notes.push("в старом боте тариф видели только новые клиенты".to_string()),
            "EXISTING" => {
                notes.push("в старом боте тариф видели только клиенты с подпиской".to_string())
            }
            "INVITED" => {
                notes.push("в старом боте тариф видели только приглашённые клиенты".to_string())
            }
            _ => {}
        }
        let device_limit = row.i64("device_limit").unwrap_or(1);
        let included = if device_limit <= 0 {
            notes.push(format!(
                "в старом боте устройств было без ограничения — стоит {MAX_DEVICES}, поправьте при необходимости"
            ));
            MAX_DEVICES
        } else {
            u32::try_from(device_limit)
                .unwrap_or(MAX_DEVICES)
                .min(MAX_DEVICES)
        };
        let traffic_gb = row.i64("traffic_limit").unwrap_or(0).max(0);
        let reset =
            parse_reset(row.text("traffic_limit_strategy")).unwrap_or(TrafficReset::NoReset);
        devices.insert(old_id.clone(), included);
        legacy.tariffs.push(LTariff {
            category_old_id: MAIN_CATEGORY.into(),
            name,
            description: row.string("description"),
            sort: row.i64("order_index").unwrap_or(0) as i32,
            on_sale: row.bool("is_active").unwrap_or(true) && !trial && !private,
            currency: legacy.currency,
            periods,
            devices_included: included,
            extra_devices_max: 0,
            extra_device_monthly_minor: None,
            traffic_limit_bytes: gb_to_bytes(traffic_gb),
            traffic_reset: reset,
            squads: uuids(row.list("internal_squads")),
            external_squad: external_squad(&row),
            note: (!notes.is_empty()).then(|| notes.join("; ")),
            old_id,
        });
    }
    if !legacy.tariffs.is_empty() {
        legacy.categories.push(LCategory {
            old_id: MAIN_CATEGORY.into(),
            name: "Тарифы".into(),
            sort: 0,
        });
    }
    devices
}

fn clients(dump: &Dump, legacy: &mut Legacy, users: &Users) -> HashSet<String> {
    let mut google: HashMap<String, String> = HashMap::new();
    let mut other_logins = 0u64;
    for row in dump.rows("user_oauth_providers") {
        let (Some(user), Some(provider), Some(id)) = (
            row.string("user_id"),
            row.text("provider"),
            row.string("provider_id"),
        ) else {
            continue;
        };
        if provider.eq_ignore_ascii_case("google") {
            google.insert(user, id);
        } else {
            other_logins += 1;
        }
    }
    let mut referrers: HashMap<String, String> = HashMap::new();
    for row in dump.rows("referrals") {
        if let (Some(referred), Some(referrer)) = (
            users.of(&row, "referred_id", "referred_telegram_id"),
            users.of(&row, "referrer_id", "referrer_telegram_id"),
        ) {
            referrers.insert(referred, referrer);
        }
    }
    let links: HashMap<String, (String, Option<String>)> = dump
        .rows("ad_links")
        .filter_map(|r| Some((r.string("id")?, (r.string("code")?, r.string("name")))))
        .collect();
    let with_sub: HashSet<String> = dump
        .rows("subscriptions")
        .filter_map(|r| users.of(&r, "user_id", "user_telegram_id"))
        .collect();
    let mut kept = HashSet::new();
    let mut seen_tg = HashSet::new();
    let mut system = 0u64;
    let mut points_clients = 0u64;
    let mut points_total = 0i64;
    let mut discounts = 0u64;
    let mut passwords = 0u64;
    for row in dump.rows("users") {
        let Some(old_id) = row.string("id") else {
            continue;
        };
        if row
            .text("role")
            .is_some_and(|r| r.eq_ignore_ascii_case("SYSTEM"))
        {
            system += 1;
            continue;
        }
        let telegram_id = row.i64("telegram_id").filter(|id| *id > 0);
        if let Some(tg) = telegram_id
            && !seen_tg.insert(tg)
        {
            legacy.problem(
                "duplicate_telegram",
                "client",
                Some(old_id),
                Some(format!("Telegram {tg}")),
                "Этот Telegram ID уже есть у другого пользователя в выгрузке — пропущен",
            );
            continue;
        }
        let raw_hash = row.text("password_hash");
        let hash = password_hash(raw_hash);
        if raw_hash.is_some() && hash.is_none() {
            passwords += 1;
        }
        let utm = row
            .string("ad_link_id")
            .and_then(|id| links.get(&id))
            .map(|(code, name)| Utm {
                source: name.clone(),
                medium: None,
                campaign: Some(code.clone()),
                content: None,
                term: None,
            });
        let client = LClient {
            telegram_id,
            username: clean_username(row.text("username")),
            first_name: row.string("name"),
            last_name: None,
            email: clean_email(row.text("email")),
            email_raw: clean_email(row.text("email")).and(row.string("email")),
            email_verified: row.flag("is_email_verified"),
            password_hash: hash,
            google_id: google.remove(&old_id),
            apple_id: None,
            locale: locale_of(row.text("language")),
            blocked: row.flag("is_blocked"),
            block_reason: None,
            bot_unreachable: row.flag("is_bot_blocked"),
            balance_minor: 0,
            balance_mixed: false,
            referral_code: row.string("referral_code"),
            referrer_old_id: referrers.remove(&old_id),
            referral_percent: None,
            utm,
            trial_used: row.bool("is_trial_available").is_some_and(|a| !a)
                || with_sub.contains(&old_id),
            created_at: row.time("created_at"),
            old_id: old_id.clone(),
        };
        let who = client.who();
        let points = row.i64("points").unwrap_or(0);
        if points > 0 {
            points_clients += 1;
            points_total += points;
            legacy.problem(
                "referral_points",
                "client",
                Some(old_id.clone()),
                Some(who.clone()),
                format!("Баллов реферальной программы: {points} — в новой системе баллов нет, не перенесены"),
            );
        }
        let discount = row.i64("personal_discount").unwrap_or(0);
        if discount > 0 {
            discounts += 1;
            legacy.problem(
                "personal_discount",
                "client",
                Some(old_id.clone()),
                Some(who),
                format!(
                    "Личная скидка {discount}% — в новой системе личных скидок нет, не перенесена"
                ),
            );
        }
        kept.insert(old_id);
        legacy.clients.push(client);
        bad_email(legacy, row.text("email"));
    }
    legacy.skip(
        "system_user",
        system,
        "служебных пользователей старого бота: это не клиенты.",
    );
    legacy.skip(
        "referral_points",
        points_clients,
        format!(
            "клиентов с баллами реферальной программы (всего {points_total}): в новой системе баллов нет. Если клиенты ждут обмена, начислите им баланс вручную — список в отчёте."
        ),
    );
    legacy.skip(
        "personal_discount",
        discounts,
        "клиентов с личной скидкой: в новой системе личных скидок нет. Если скидку обещали, выдайте промокод — список в отчёте.",
    );
    legacy.skip(
        "passwords",
        passwords,
        "паролей для входа по почте: старый бот шифровал их своим ключом. Почта перенесена, пароль клиенты зададут заново через «Забыли пароль?».",
    );
    legacy.skip(
        "oauth_other",
        other_logins,
        "входов через Яндекс и VK: в новой системе их нет. Клиенты войдут по почте или через Telegram.",
    );
    kept
}

fn subscriptions(
    dump: &Dump,
    legacy: &mut Legacy,
    users: &Users,
    kept: &HashSet<String>,
    devices: &HashMap<String, u32>,
) {
    let current: HashMap<String, String> = dump
        .rows("users")
        .filter_map(|r| Some((r.string("id")?, r.string("current_subscription_id")?)))
        .collect();
    let mut deleted = 0u64;
    let mut extra = 0u64;
    let mut per_user: BTreeMap<String, Vec<Row<'_>>> = BTreeMap::new();
    for row in dump.rows("subscriptions") {
        let (Some(_), Some(user)) = (
            row.string("id"),
            users.of(&row, "user_id", "user_telegram_id"),
        ) else {
            continue;
        };
        if !kept.contains(&user) {
            continue;
        }
        if row
            .text("status")
            .is_some_and(|s| s.eq_ignore_ascii_case("DELETED"))
        {
            deleted += 1;
            continue;
        }
        per_user.entry(user).or_default().push(row);
    }
    for (user, rows) in per_user {
        let chosen = current
            .get(&user)
            .and_then(|id| rows.iter().find(|r| r.text("id") == Some(id.as_str())))
            .or_else(|| {
                rows.iter()
                    .max_by_key(|r| (r.time("expire_at"), r.i64("id").unwrap_or(0)))
            })
            .copied();
        extra += rows.len().saturating_sub(1) as u64;
        let Some(row) = chosen else { continue };
        let Some(old_id) = row.string("id") else {
            continue;
        };
        let plan = snapshot(&row);
        let tariff = snapshot_id(&plan).filter(|id| devices.contains_key(id));
        let device_limit = row.i64("device_limit").unwrap_or(0);
        let extra_devices = match (
            tariff.as_ref().and_then(|t| devices.get(t)),
            u32::try_from(device_limit),
        ) {
            (Some(included), Ok(limit)) if limit > 0 => limit.saturating_sub(*included),
            _ => 0,
        };
        let remna_uuid = row.string("user_remna_id");
        legacy.subscriptions.push(LSubscription {
            client_old_id: user,
            tariff_old_id: tariff,
            kind: if row.flag("is_trial") {
                SubscriptionKind::Trial
            } else {
                SubscriptionKind::Paid
            },
            remna_id: remna_uuid.as_deref().and_then(|v| v.parse::<i64>().ok()),
            remna_uuid: remna_uuid.filter(|v| v.parse::<i64>().is_err()),
            short_uuid: short_uuid_from_url(row.text("url")),
            expire_at: row.time("expire_at"),
            extra_devices,
            autorenew: false,
            autorenew_days: None,
            autorenew_period_old_id: None,
            ordinal: 0,
            gifted: false,
            created_at: row.time("created_at"),
            old_id,
        });
    }
    legacy.skip(
        "subscriptions_history",
        deleted,
        "старых записей о подписках (после смены тарифа или удаления): у клиента переносится только действующая подписка.",
    );
    legacy.skip(
        "subscriptions_extra",
        extra,
        "лишних записей о подписке у одного клиента: в Remnashop у клиента одна подписка, перенесена текущая.",
    );
}

fn payments(dump: &Dump, legacy: &mut Legacy, users: &Users, devices: &HashMap<String, u32>) {
    let mut unpaid = 0u64;
    let mut test = 0u64;
    let mut free = 0u64;
    let mut unknown = 0u64;
    for row in dump.rows("transactions") {
        let (Some(old_id), Some(client)) = (
            row.string("id"),
            users.of(&row, "user_id", "user_telegram_id"),
        ) else {
            continue;
        };
        if row.flag("is_test") {
            test += 1;
            continue;
        }
        let refunded = match row
            .text("status")
            .unwrap_or("")
            .to_ascii_uppercase()
            .as_str()
        {
            "COMPLETED" => false,
            "REFUNDED" => true,
            _ => {
                unpaid += 1;
                continue;
            }
        };
        let pricing = row.json("pricing").unwrap_or(serde_json::Value::Null);
        let total = number(pricing.get("final_amount"))
            .or_else(|| number(pricing.get("amount")))
            .unwrap_or(0.0);
        let raw_currency = row.text("currency").unwrap_or("").to_ascii_uppercase();
        let (currency, amount) = if raw_currency == "XTR" {
            (PayCurrencyLegacy::Stars, total.round() as i64)
        } else if let Some(c) = currency_of(Some(raw_currency.as_str())) {
            (
                PayCurrencyLegacy::Money(c),
                minor_from_float(total).unwrap_or(0),
            )
        } else {
            unknown += 1;
            continue;
        };
        if amount <= 0 {
            free += 1;
            continue;
        }
        let plan = snapshot(&row);
        let tariff = snapshot_id(&plan).filter(|id| devices.contains_key(id));
        let days = snapshot_days(&plan);
        let label = match (&tariff, snapshot_name(&plan)) {
            (None, Some(name)) => Some(match days {
                Some(d) => format!("Тариф «{name}», {d} дн."),
                None => format!("Тариф «{name}»"),
            }),
            _ => None,
        };
        let created_at = row.time("created_at");
        legacy.payments.push(LPayment {
            client_old_id: client,
            kind: PaymentKind::Subscription,
            refunded,
            amount_minor: amount,
            currency,
            provider: provider_of(row.text("gateway_type")),
            external_id: row.string("payment_id"),
            tariff_old_id: tariff,
            days,
            subscription_old_id: None,
            label,
            created_at,
            paid_at: row.time("updated_at").or(created_at),
            old_id,
        });
    }
    legacy.skip(
        "payments_unpaid",
        unpaid,
        "неоплаченных и отменённых платежей: в истории они не нужны.",
    );
    legacy.skip(
        "payments_test",
        test,
        "тестовых платежей: в истории их нет.",
    );
    legacy.skip(
        "payments_free",
        free,
        "бесплатных выдач: в истории платежей их нет, подписки переносятся.",
    );
    legacy.skip(
        "payments_currency",
        unknown,
        "оплат в незнакомой валюте: в истории их нет.",
    );
}

fn promos(dump: &Dump, legacy: &mut Legacy, users: &Users, devices: &HashMap<String, u32>) {
    let mut unsupported = 0u64;
    for row in dump.rows("promocodes") {
        let (Some(old_id), Some(raw)) = (row.string("id"), row.string("code")) else {
            continue;
        };
        let Some(code) = promo_code(&raw) else {
            unsupported += 1;
            legacy.problem(
                "promo_code_format",
                "promo",
                Some(old_id),
                Some(raw),
                "Код промокода должен быть из латинских букв и цифр, от 3 до 32 знаков — не перенесён",
            );
            continue;
        };
        let kind = row.text("reward_type").unwrap_or("").to_ascii_uppercase();
        let reward = row.i64("reward");
        let plan = snapshot(&row);
        let mut notes = Vec::new();
        let mut tariff = None;
        let value = match kind.as_str() {
            "DURATION" => reward
                .and_then(|d| u32::try_from(d).ok())
                .filter(|d| (1..=3650).contains(d))
                .map(PromoValue::Days),
            "SUBSCRIPTION" => {
                tariff = snapshot_id(&plan).filter(|id| devices.contains_key(id));
                if let Some(name) = snapshot_name(&plan) {
                    notes.push(format!("в старом боте промокод выдавал подписку «{name}»"));
                }
                snapshot_days(&plan)
                    .filter(|d| *d <= 3650)
                    .map(PromoValue::Days)
            }
            "PURCHASE_DISCOUNT" | "PERSONAL_DISCOUNT" => {
                if kind == "PERSONAL_DISCOUNT" {
                    notes.push(
                        "в старом боте скидка оставалась у клиента навсегда, здесь — на одну покупку"
                            .to_string(),
                    );
                }
                reward
                    .filter(|p| (1..=100).contains(p))
                    .map(|p| PromoValue::Percent(p as u8))
            }
            _ => None,
        };
        let Some(value) = value else {
            unsupported += 1;
            legacy.problem(
                "promo_unsupported",
                "promo",
                Some(old_id),
                Some(code),
                match kind.as_str() {
                    "TRAFFIC" => "Промокод добавлял трафик — в новой системе промокоды дают скидку или дни",
                    "DEVICES" => {
                        "Промокод добавлял устройства — в новой системе промокоды дают скидку или дни"
                    }
                    _ => "Промокод без скидки и без дней",
                },
            );
            continue;
        };
        let availability = row
            .text("availability")
            .unwrap_or("ALL")
            .to_ascii_uppercase();
        match availability.as_str() {
            "EXISTING" => {
                notes.push("в старом боте промокод был только для клиентов с подпиской".into())
            }
            "INVITED" => notes.push("в старом боте промокод был только для приглашённых".into()),
            _ => {}
        }
        let valid_until = row.time("expires_at").or_else(|| {
            let days = row.i64("lifetime").filter(|d| *d > 0)?;
            Some(row.time("created_at")? + Duration::days(days))
        });
        let max_uses = row
            .i64("max_activations")
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0);
        legacy.promos.push(LPromo {
            code,
            value,
            max_uses,
            per_client: if row.flag("is_reusable") { 1000 } else { 1 },
            valid_from: None,
            valid_until,
            active: row.bool("is_active").unwrap_or(true),
            first_purchase_only: availability == "NEW",
            tariff_old_id: tariff,
            note: (!notes.is_empty()).then(|| notes.join("; ")),
            old_id,
        });
    }
    for row in dump.rows("promocode_activations") {
        let (Some(old_id), Some(promo), Some(user)) = (
            row.string("id"),
            row.string("promocode_id"),
            users.of(&row, "user_id", "user_telegram_id"),
        ) else {
            continue;
        };
        legacy.promo_uses.push(LPromoUse {
            old_id,
            promo_old_id: promo,
            client_old_id: user,
            used_at: row.time("activated_at"),
        });
    }
    legacy.skip(
        "promo_unsupported",
        unsupported,
        "промокодов другого вида (на трафик, на устройства) или с неподходящим кодом: в новой системе промокоды дают скидку или дни. Список в отчёте.",
    );
}

fn leftovers(dump: &Dump, legacy: &mut Legacy) {
    let gateways: Vec<String> = dump
        .rows("payment_gateways")
        .filter(|r| r.flag("is_active"))
        .filter_map(|r| r.text("type").map(gateway_title))
        .collect();
    legacy.skip(
        "payment_keys",
        gateways.len() as u64,
        format!(
            "платёжек было подключено в старом боте ({}): ключи от них не переносятся. Подключите платёжки заново в «Настройки → Оплата».",
            gateways.join(", ")
        ),
    );
    let referral = dump
        .rows("settings")
        .find_map(|r| r.json("referral"))
        .filter(|r| r.get("enable").and_then(serde_json::Value::as_bool) != Some(false));
    if let Some(settings) = referral {
        let reward = settings
            .pointer("/reward/type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_ascii_uppercase);
        let what = match reward.as_deref() {
            Some("EXTRA_DAYS") => "за приглашённых давала дни",
            Some("POINTS") => "за приглашённых давала баллы",
            _ => "была включена",
        };
        legacy.skip(
            "referral_settings",
            1,
            format!("настройка реферальной программы: в старом боте она {what}. Условия рефералов задайте сами в «Рефералы»."),
        );
    }
}

pub fn map(dump: &Dump) -> Legacy {
    let mut legacy = Legacy::new(Source::Remnashop, decide_currency(dump));
    legacy.dumped_at = dump.dumped_at;
    let users = Users::new(dump);
    let devices = tariffs(dump, &mut legacy);
    let kept = clients(dump, &mut legacy, &users);
    subscriptions(dump, &mut legacy, &users, &kept, &devices);
    payments(dump, &mut legacy, &users, &devices);
    promos(dump, &mut legacy, &users, &devices);
    leftovers(dump, &mut legacy);
    legacy
}
