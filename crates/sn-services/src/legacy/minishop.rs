use super::model::*;
use super::table::{Dump, Row, minor_from_float, parse_time};
use serde_json::Value;
use sn_domain::money::Currency;
use sn_domain::subscription::SubscriptionKind;
use sn_domain::traffic::TrafficReset;
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

pub const TABLES: &[&str] = &[
    "users",
    "subscriptions",
    "payments",
    "promo_codes",
    "promo_code_activations",
    "user_balance_ledger_entries",
    "partner_profiles",
    "partner_ledger_entries",
    "partner_withdrawals",
    "support_tickets",
    "support_ticket_messages",
    "user_payment_methods",
    "user_billing",
    "app_setting_overrides",
];

pub const MAIN_CATEGORY: &str = "minishop:main";
pub const TARIFFS_FILE: &str = "database/tariffs.json";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const MAX_DEVICES: u32 = 100;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

pub fn looks_like(dump: &Dump) -> bool {
    let manifest = dump
        .files
        .get(MANIFEST_FILE)
        .is_some_and(|m| m.contains("remnawave-minishop"));
    dump.table("users").is_some()
        && (manifest
            || dump
                .table("users")
                .is_some_and(|t| t.has("panel_user_uuid"))
            || dump
                .table("subscriptions")
                .is_some_and(|t| t.has("panel_user_uuid")))
}

fn currency_of(raw: Option<&str>) -> Option<Currency> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "rub" | "rur" | "₽" => Some(Currency::Rub),
        "usd" | "$" => Some(Currency::Usd),
        _ => None,
    }
}

fn stars(raw: Option<&str>) -> bool {
    raw.is_some_and(|r| {
        let r = r.trim();
        r.eq_ignore_ascii_case("xtr") || r.eq_ignore_ascii_case("stars")
    })
}

fn overrides(dump: &Dump) -> HashMap<String, String> {
    dump.rows("app_setting_overrides")
        .filter_map(|r| Some((r.string("key")?.to_ascii_uppercase(), r.string("value")?)))
        .collect()
}

fn decide_currency(dump: &Dump, config: &Value, settings: &HashMap<String, String>) -> Currency {
    for key in ["USER_BALANCE_CURRENCY", "DEFAULT_CURRENCY_SYMBOL"] {
        if let Some(found) = settings
            .get(key)
            .and_then(|v| currency_of(Some(v.trim_matches('"'))))
        {
            return found;
        }
    }
    if let Some(found) = currency_of(config.get("default_currency").and_then(Value::as_str)) {
        return found;
    }
    let mut tally: HashMap<Currency, usize> = HashMap::new();
    for row in dump.rows("payments") {
        if row
            .text("status")
            .is_some_and(|s| s.eq_ignore_ascii_case("succeeded"))
            && let Some(c) = currency_of(row.text("currency"))
        {
            *tally.entry(c).or_default() += 1;
        }
    }
    tally
        .into_iter()
        .max_by_key(|(c, n)| (*n, *c == Currency::Rub))
        .map_or(Currency::Rub, |(c, _)| c)
}

fn months_to_days(months: u32) -> u32 {
    (months / 12) * 365 + (months % 12) * 30
}

fn localized(value: Option<&Value>) -> Option<String> {
    let map = value?.as_object()?;
    ["ru", "en"]
        .iter()
        .find_map(|lang| map.get(*lang))
        .or_else(|| map.values().next())
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn number(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
        .filter(|v| v.is_finite())
}

fn uuids<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<Uuid> {
    let mut out: Vec<Uuid> = values
        .into_iter()
        .filter_map(|v| Uuid::parse_str(v.trim()).ok())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn period_text(value: &Value) -> Option<String> {
    value
        .as_f64()
        .map(|v| v.to_string())
        .or_else(|| value.as_str().map(str::to_string))
}

fn gb_bytes(gb: f64) -> Option<u64> {
    (gb > 0.0 && gb.is_finite()).then(|| (gb * GIB).round() as u64)
}

fn tariffs(config: &Value, legacy: &mut Legacy) -> HashMap<String, (String, u32)> {
    let mut keys = HashMap::new();
    let Some(list) = config.get("tariffs").and_then(Value::as_array) else {
        return keys;
    };
    let schema = config.get("schema_version").and_then(Value::as_i64);
    let top_unit = config
        .get("period_unit")
        .and_then(Value::as_str)
        .unwrap_or(if schema == Some(2) { "day" } else { "month" })
        .to_ascii_lowercase();
    let currency = match legacy.currency {
        Currency::Rub => "rub",
        Currency::Usd => "usd",
    };
    for (index, tariff) in list.iter().enumerate() {
        let Some(key) = tariff
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|k| !k.is_empty())
        else {
            continue;
        };
        let name = localized(tariff.get("names")).unwrap_or_else(|| key.to_string());
        let unit = tariff
            .get("period_unit")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase)
            .unwrap_or_else(|| top_unit.clone());
        let to_days = |raw: &str| -> Option<u32> {
            let value = raw.trim().parse::<f64>().ok()?;
            if !(1.0..=36_500.0).contains(&value) {
                return None;
            }
            let value = value as u32;
            Some(if unit == "month" {
                months_to_days(value)
            } else {
                value
            })
        };
        let mut notes = Vec::new();
        let by_traffic = tariff
            .get("billing_model")
            .and_then(Value::as_str)
            .is_some_and(|m| m.eq_ignore_ascii_case("traffic"));
        if by_traffic {
            notes.push(
                "в старом боте тариф продавал гигабайты без срока — здесь подписка всегда со сроком, тариф снят с продажи".to_string(),
            );
        }
        let price_map = tariff
            .pointer(&format!("/prices/{currency}"))
            .or_else(|| tariff.get(format!("prices_{currency}")))
            .and_then(Value::as_object);
        let star_prices = tariff
            .pointer("/prices/stars")
            .or_else(|| tariff.get("prices_stars"))
            .and_then(Value::as_object)
            .is_some_and(|m| m.values().any(|v| number(Some(v)).is_some_and(|p| p > 0.0)));
        let enabled: Option<HashSet<u32>> = tariff
            .get("enabled_periods")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
            .map(|a| {
                a.iter()
                    .filter_map(period_text)
                    .filter_map(|v| to_days(&v))
                    .collect()
            });
        let mut periods: Vec<LPeriod> = Vec::new();
        if let Some(prices) = price_map {
            for (raw, price) in prices {
                let Some(days) = to_days(raw) else { continue };
                if enabled.as_ref().is_some_and(|e| !e.contains(&days))
                    || periods.iter().any(|p| p.days == days)
                {
                    continue;
                }
                periods.push(LPeriod {
                    old_id: None,
                    days,
                    price_minor: number(Some(price))
                        .and_then(minor_from_float)
                        .filter(|p| *p > 0),
                    sort: days as i32,
                });
            }
        }
        periods.sort_by_key(|p| p.days);
        if periods.is_empty() && !by_traffic {
            notes.push(format!(
                "в старом боте у тарифа нет цен в {} — задайте сроки и цены сами",
                legacy.currency.code()
            ));
        }
        if star_prices {
            notes.push("цены в звёздах задайте сами".to_string());
        }
        let limit = tariff.get("hwid_device_limit").and_then(Value::as_i64);
        let included = match limit {
            Some(n) if n > 0 => u32::try_from(n).unwrap_or(MAX_DEVICES).min(MAX_DEVICES),
            _ => {
                notes.push(format!(
                    "в старом боте устройств было без ограничения — стоит {MAX_DEVICES}, поправьте при необходимости"
                ));
                MAX_DEVICES
            }
        };
        let addon = tariff.pointer("/checkout_addons/devices");
        let addon_on = addon
            .and_then(|a| a.get("enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let extra_max = addon
            .filter(|_| addon_on)
            .and_then(|a| a.get("max_extra_devices"))
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0)
            .min(MAX_DEVICES);
        let extra_price = addon
            .filter(|_| addon_on)
            .and_then(|a| number(a.get("price_per_device")))
            .and_then(minor_from_float)
            .filter(|p| *p > 0);
        let traffic = number(tariff.get("monthly_gb")).and_then(gb_bytes);
        let reset = match parse_reset(tariff.get("traffic_limit_strategy").and_then(Value::as_str))
        {
            Some(reset) => reset,
            None if traffic.is_some() => {
                notes.push(
                    "сброс трафика брался из общих настроек старого бота — стоит «раз в месяц», проверьте"
                        .to_string(),
                );
                TrafficReset::Month
            }
            None => TrafficReset::NoReset,
        };
        if tariff
            .get("premium_squad_uuids")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
        {
            notes.push("премиум-серверов с отдельным трафиком здесь нет — перенесены только обычные серверы".to_string());
        }
        let squads = uuids(
            tariff
                .get("squad_uuids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        );
        let on_sale = tariff
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true)
            && !by_traffic;
        keys.insert(key.to_string(), (key.to_string(), included));
        for alias in tariff
            .get("legacy_keys")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            keys.entry(alias.trim().to_string())
                .or_insert_with(|| (key.to_string(), included));
        }
        legacy.tariffs.push(LTariff {
            old_id: key.to_string(),
            category_old_id: MAIN_CATEGORY.into(),
            name,
            description: localized(tariff.get("descriptions")),
            sort: index as i32,
            on_sale,
            currency: legacy.currency,
            periods,
            devices_included: included,
            extra_devices_max: if extra_price.is_some() { extra_max } else { 0 },
            extra_device_monthly_minor: extra_price.filter(|_| extra_max > 0),
            traffic_limit_bytes: traffic,
            traffic_reset: reset,
            squads,
            external_squad: None,
            note: (!notes.is_empty()).then(|| notes.join("; ")),
        });
    }
    if !legacy.tariffs.is_empty() {
        legacy.categories.push(LCategory {
            old_id: MAIN_CATEGORY.into(),
            name: "Тарифы".into(),
            sort: 0,
        });
    }
    keys
}

fn scaled(amount: i64, scale: i64) -> i64 {
    match scale {
        2 => amount,
        s if s < 2 => amount.saturating_mul(10i64.pow((2 - s).min(6) as u32)),
        s => amount / 10i64.pow((s - 2).min(12) as u32),
    }
}

fn balances(dump: &Dump) -> HashMap<String, BTreeMap<String, i64>> {
    let mut out: HashMap<String, BTreeMap<String, i64>> = HashMap::new();
    for row in dump.rows("user_balance_ledger_entries") {
        if row
            .text("state")
            .is_some_and(|s| !s.eq_ignore_ascii_case("posted"))
        {
            continue;
        }
        let (Some(user), Some(currency), Some(amount)) = (
            row.string("user_id"),
            row.text("currency"),
            row.i64("amount_minor"),
        ) else {
            continue;
        };
        let minor = scaled(amount, row.i64("currency_scale").unwrap_or(2));
        *out.entry(user)
            .or_default()
            .entry(currency.to_ascii_uppercase())
            .or_default() += minor;
    }
    out
}

fn partners(dump: &Dump) -> (HashMap<String, String>, HashMap<String, i32>) {
    let mut owners = HashMap::new();
    let mut percents = HashMap::new();
    for row in dump.rows("partner_profiles") {
        let (Some(partner), Some(user)) = (row.string("partner_id"), row.string("user_id")) else {
            continue;
        };
        owners.insert(partner, user.clone());
        if row
            .text("status")
            .is_none_or(|s| s.eq_ignore_ascii_case("active"))
            && let Some(bps) = row
                .i64("commission_bps")
                .filter(|b| (0..=10_000).contains(b))
        {
            percents.insert(user, (bps as f64 / 100.0).round() as i32);
        }
    }
    (owners, percents)
}

fn clients(
    dump: &Dump,
    legacy: &mut Legacy,
    with_sub: &HashSet<String>,
    percents: &HashMap<String, i32>,
) -> HashSet<String> {
    let money = balances(dump);
    let code = legacy.currency.code();
    let telegram_column = dump.table("users").is_some_and(|t| t.has("telegram_id"));
    let mut kept = HashSet::new();
    let mut seen_tg = HashSet::new();
    let mut passwords = 0u64;
    let mut foreign = 0u64;
    for row in dump.rows("users") {
        let Some(old_id) = row.string("user_id") else {
            continue;
        };
        let telegram_id = if telegram_column {
            row.i64("telegram_id")
        } else {
            row.i64("user_id")
        }
        .filter(|id| *id > 0);
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
        let wallet = money.get(&old_id);
        let positive: Vec<(&String, &i64)> = wallet
            .into_iter()
            .flatten()
            .filter(|(_, v)| **v > 0)
            .collect();
        let own = wallet
            .and_then(|w| w.get(code))
            .copied()
            .unwrap_or(0)
            .max(0);
        let others: Vec<String> = positive
            .iter()
            .filter(|(c, _)| c.as_str() != code)
            .map(|(c, v)| format!("{}.{:02} {c}", **v / 100, **v % 100))
            .collect();
        let client = LClient {
            telegram_id,
            username: clean_username(row.text("username")),
            first_name: row.string("first_name"),
            last_name: row.string("last_name"),
            email: clean_email(row.text("email")),
            email_raw: clean_email(row.text("email")).and(row.string("email")),
            email_verified: row.text("email_verified_at").is_some(),
            password_hash: hash,
            google_id: None,
            apple_id: None,
            locale: locale_of(row.text("language_code")),
            blocked: row.flag("is_banned"),
            block_reason: None,
            bot_unreachable: row
                .text("telegram_notifications_status")
                .is_some_and(|s| s.eq_ignore_ascii_case("blocked")),
            balance_minor: own,
            balance_mixed: false,
            referral_code: row.string("referral_code"),
            referrer_old_id: row.string("referred_by_id"),
            referral_percent: percents.get(&old_id).copied(),
            utm: None,
            trial_used: with_sub.contains(&old_id),
            created_at: row.time("registration_date"),
            old_id: old_id.clone(),
        };
        if !others.is_empty() {
            foreign += 1;
            legacy.problem(
                "balance_currency",
                "client",
                Some(old_id.clone()),
                Some(client.who()),
                format!(
                    "Баланс в другой валюте ({}) не перенесён — в установке одна валюта, решите вручную",
                    others.join(", ")
                ),
            );
        }
        kept.insert(old_id);
        legacy.clients.push(client);
        bad_email(legacy, row.text("email"));
    }
    legacy.skip(
        "passwords",
        passwords,
        "паролей для входа по почте: старый бот хранил их в своём формате. Почта перенесена, пароль клиенты зададут заново через «Забыли пароль?».",
    );
    legacy.skip(
        "balance_currency",
        foreign,
        "клиентов с балансом в другой валюте: в установке одна валюта, такой баланс не переносим. Суммы в отчёте, решите вручную.",
    );
    kept
}

fn subscriptions(
    dump: &Dump,
    legacy: &mut Legacy,
    kept: &HashSet<String>,
    tariffs: &HashMap<String, (String, u32)>,
) -> BTreeMap<String, u64> {
    let panel: HashMap<String, String> = dump
        .rows("users")
        .filter_map(|r| Some((r.string("user_id")?, r.string("panel_user_uuid")?)))
        .collect();
    let cards: HashSet<String> = dump
        .rows("user_payment_methods")
        .filter_map(|r| r.string("user_id"))
        .chain(
            dump.rows("user_billing")
                .filter(|r| r.text("yookassa_payment_method_id").is_some())
                .filter_map(|r| r.string("user_id")),
        )
        .collect();
    let mut per_user: BTreeMap<String, Vec<Row<'_>>> = BTreeMap::new();
    for row in dump.rows("subscriptions") {
        let (Some(_), Some(user)) = (row.string("subscription_id"), row.string("user_id")) else {
            continue;
        };
        if kept.contains(&user) {
            per_user.entry(user).or_default().push(row);
        }
    }
    let mut history = 0u64;
    let mut renewals = 0u64;
    let mut unknown: BTreeMap<String, u64> = BTreeMap::new();
    for (user, rows) in per_user {
        history += rows.len().saturating_sub(1) as u64;
        let Some(row) = rows
            .iter()
            .max_by_key(|r| {
                (
                    r.flag("is_active"),
                    r.time("end_date"),
                    r.i64("subscription_id").unwrap_or(0),
                )
            })
            .copied()
        else {
            continue;
        };
        let Some(old_id) = row.string("subscription_id") else {
            continue;
        };
        let tariff = row.string("tariff_key");
        let known = tariff.as_ref().and_then(|t| tariffs.get(t));
        if let (Some(key), None) = (&tariff, known) {
            *unknown.entry(key.clone()).or_default() += 1;
        }
        let provider = row.text("provider").unwrap_or("").to_ascii_lowercase();
        let kind = match provider.as_str() {
            "trial" => SubscriptionKind::Trial,
            "referral" | "registration" | "bonus" | "promo" => SubscriptionKind::Promo,
            "gift" | "admin_gift" => SubscriptionKind::Gift,
            _ => SubscriptionKind::Paid,
        };
        if row.flag("auto_renew_enabled") && cards.contains(&user) {
            renewals += 1;
        }
        let remna = row
            .string("panel_user_uuid")
            .or_else(|| panel.get(&user).cloned());
        let remna_id = remna.as_deref().and_then(|v| v.trim().parse::<i64>().ok());
        legacy.subscriptions.push(LSubscription {
            client_old_id: user,
            tariff_old_id: known.map(|(key, _)| key.clone()),
            kind,
            remna_id,
            remna_uuid: remna.filter(|_| remna_id.is_none()),
            short_uuid: row.string("install_share_panel_short_uuid"),
            expire_at: row.time("end_date"),
            extra_devices: row
                .i64("extra_hwid_devices")
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0),
            autorenew: false,
            autorenew_days: None,
            autorenew_period_old_id: None,
            ordinal: 0,
            gifted: false,
            created_at: row.time("start_date"),
            old_id,
        });
    }
    legacy.skip(
        "subscriptions_history",
        history,
        "старых записей о подписках (прошлые периоды): у клиента переносится последняя подписка.",
    );
    legacy.skip(
        "autorenew_cards",
        renewals,
        "подписок продлевались с сохранённой карты: карты привязаны к платёжке старого бота и не переносятся. Клиенты включат автопродление заново.",
    );
    unknown
}

fn tariff_notes(legacy: &mut Legacy, has_file: bool, unknown: &BTreeMap<String, u64>) {
    let names: Vec<String> = unknown.keys().cloned().collect();
    if !has_file {
        let list = if names.is_empty() {
            String::new()
        } else {
            format!(" ({})", names.join(", "))
        };
        legacy.skip(
            "tariffs_file",
            names.len().max(1) as u64,
            format!(
                "тарифов нет в файле{list}: Minishop хранит тарифы не в базе, а в файле tariffs.json, и в этой копии его нет. Подписки перенесены без тарифа — заведите тарифы вручную в «Тарифы» или загрузите полную копию из админки Minishop, в ней этот файл есть."
            ),
        );
    } else if !names.is_empty() {
        legacy.skip(
            "tariffs_missing",
            names.len() as u64,
            format!(
                "тарифов ({}) нет в файле tariffs.json, хотя подписки на них есть: такие подписки перенесены без тарифа. Заведите эти тарифы вручную в «Тарифы».",
                names.join(", ")
            ),
        );
    }
}

fn provider_of(raw: Option<&str>, funding: Option<&str>) -> String {
    if funding.is_some_and(|f| f.to_ascii_lowercase().contains("user_balance")) {
        return "balance".into();
    }
    let raw = raw.unwrap_or("").trim().to_ascii_lowercase();
    let base = raw
        .strip_suffix("_subscription")
        .or_else(|| raw.strip_suffix("_crypto"))
        .unwrap_or(&raw);
    match base {
        "telegram_stars" | "stars" => "stars".into(),
        "" => "unknown".into(),
        other => other.to_string(),
    }
}

fn payments(
    dump: &Dump,
    legacy: &mut Legacy,
    kept: &HashSet<String>,
    tariffs: &HashMap<String, (String, u32)>,
) {
    let mut unpaid = 0u64;
    let mut free = 0u64;
    let mut unknown = 0u64;
    for row in dump.rows("payments") {
        let (Some(old_id), Some(client)) = (row.string("payment_id"), row.string("user_id")) else {
            continue;
        };
        if !kept.contains(&client) {
            continue;
        }
        let refunded = match row
            .text("status")
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "succeeded" => false,
            "refunded" | "reversed" => true,
            _ => {
                unpaid += 1;
                continue;
            }
        };
        let funding = row.text("funding_source");
        if funding.is_some_and(|f| matches!(f, "admin_grant" | "checkout_promo")) {
            free += 1;
            continue;
        }
        let raw_currency = row.text("currency");
        let total = row.f64("amount").unwrap_or(0.0);
        let (currency, amount) = if stars(raw_currency) {
            (PayCurrencyLegacy::Stars, total.round() as i64)
        } else if let Some(c) = currency_of(raw_currency) {
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
        let mode = row.text("sale_mode").unwrap_or("").to_ascii_lowercase();
        let days = row
            .i64("subscription_duration_days")
            .and_then(|d| u32::try_from(d).ok())
            .filter(|d| *d > 0)
            .or_else(|| {
                row.i64("subscription_duration_months")
                    .and_then(|m| u32::try_from(m).ok())
                    .filter(|m| *m > 0)
                    .map(months_to_days)
            });
        let (kind, label) = match mode.as_str() {
            "balance_topup" => (PaymentKind::TopUp, None),
            "topup" | "traffic" | "traffic_package" | "premium_topup" => {
                (PaymentKind::Other, Some("Докупка трафика".to_string()))
            }
            "hwid_devices_renewal" => (PaymentKind::Other, Some("Устройства".to_string())),
            "gift" => (PaymentKind::Other, Some("Подарочная подписка".to_string())),
            _ => (PaymentKind::Subscription, None),
        };
        let created_at = row.time("created_at");
        legacy.payments.push(LPayment {
            client_old_id: client,
            kind,
            refunded,
            amount_minor: amount,
            currency,
            provider: provider_of(row.text("provider"), funding),
            external_id: row
                .string("provider_payment_id")
                .or_else(|| row.string("yookassa_payment_id")),
            tariff_old_id: row
                .string("tariff_key")
                .and_then(|k| tariffs.get(&k))
                .map(|(key, _)| key.clone()),
            days: days.filter(|_| kind == PaymentKind::Subscription),
            subscription_old_id: row.string("renewal_subscription_id"),
            label: label.or_else(|| {
                (kind == PaymentKind::Subscription)
                    .then(|| row.string("description"))
                    .flatten()
                    .filter(|_| row.text("tariff_key").is_none())
                    .map(|d| d.chars().take(120).collect())
            }),
            created_at,
            paid_at: row
                .time("fulfilled_at")
                .or_else(|| row.time("updated_at"))
                .or(created_at),
            old_id,
        });
    }
    legacy.skip(
        "payments_unpaid",
        unpaid,
        "неоплаченных, отменённых и просроченных счетов: в истории они не нужны.",
    );
    legacy.skip(
        "payments_free",
        free,
        "выдач без оплаты (пробные, подарки от админа, промокоды): в истории платежей их нет, подписки переносятся.",
    );
    legacy.skip(
        "payments_currency",
        unknown,
        "оплат в незнакомой валюте: в истории их нет.",
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

fn promos(dump: &Dump, legacy: &mut Legacy, tariffs: &HashMap<String, (String, u32)>) {
    let mut unsupported = 0u64;
    for row in dump.rows("promo_codes") {
        let (Some(old_id), Some(raw)) = (row.string("promo_code_id"), row.string("code")) else {
            continue;
        };
        if row.text("archived_at").is_some() {
            continue;
        }
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
        let percent = row
            .f64("discount_percent")
            .filter(|p| *p > 0.0)
            .map(|p| p.round().clamp(1.0, 100.0) as u8);
        let days = row
            .i64("bonus_days")
            .and_then(|d| u32::try_from(d).ok())
            .filter(|d| (1..=3650).contains(d));
        let mut notes = Vec::new();
        let value = match (percent, days) {
            (Some(p), Some(d)) => {
                notes.push(format!(
                    "в старом боте промокод давал ещё и {d} дн. — они не переносятся"
                ));
                Some(PromoValue::Percent(p))
            }
            (Some(p), None) => Some(PromoValue::Percent(p)),
            (None, Some(d)) => Some(PromoValue::Days(d)),
            (None, None) => None,
        };
        let Some(value) = value else {
            unsupported += 1;
            legacy.problem(
                "promo_unsupported",
                "promo",
                Some(old_id),
                Some(code),
                "Промокод давал трафик или множитель — в новой системе промокоды дают скидку или дни",
            );
            continue;
        };
        if ["regular_traffic_gb", "premium_traffic_gb"]
            .iter()
            .any(|c| row.f64(c).is_some_and(|v| v > 0.0))
        {
            notes.push("трафик из старого промокода не переносится".to_string());
        }
        if row.text("user_id").is_some() {
            notes.push("в старом боте это был личный промокод одного клиента".to_string());
        }
        let applies = row.text("applies_to").unwrap_or("all").to_ascii_lowercase();
        let tariff = row
            .string("subscription_tariff_key")
            .and_then(|k| tariffs.get(&k))
            .map(|(key, _)| key.clone());
        legacy.promos.push(LPromo {
            code,
            value,
            max_uses: row
                .i64("max_activations")
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v > 0),
            per_client: 1,
            valid_from: None,
            valid_until: row.time("valid_until"),
            active: row.bool("is_active").unwrap_or(true),
            first_purchase_only: applies == "new" || applies == "first_purchase",
            tariff_old_id: tariff,
            note: (!notes.is_empty()).then(|| notes.join("; ")),
            old_id,
        });
    }
    for row in dump.rows("promo_code_activations") {
        let (Some(old_id), Some(promo), Some(user)) = (
            row.string("activation_id"),
            row.string("promo_code_id"),
            row.string("user_id"),
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
        "промокодов другого вида (на трафик, множители) или с неподходящим кодом: в новой системе промокоды дают скидку или дни. Список в отчёте.",
    );
}

fn tickets(dump: &Dump, legacy: &mut Legacy) {
    let mut messages: HashMap<String, Vec<LMessage>> = HashMap::new();
    for row in dump.rows("support_ticket_messages") {
        let (Some(old_id), Some(ticket)) = (row.string("message_id"), row.string("ticket_id"))
        else {
            continue;
        };
        if row.flag("is_internal_note") {
            continue;
        }
        let text = row.raw("body").unwrap_or("").trim().to_string();
        let image = row.text("image_id").is_some();
        let body = match (text.is_empty(), image) {
            (true, false) => continue,
            (true, true) => "[картинка осталась в старом боте]".to_string(),
            (false, true) => format!("{text}\n[картинка осталась в старом боте]"),
            (false, false) => text,
        };
        let from_support = !row
            .text("author_role")
            .is_some_and(|r| r.eq_ignore_ascii_case("user"));
        messages.entry(ticket).or_default().push(LMessage {
            old_id,
            from_support,
            body,
            read: !from_support || row.text("read_by_user_at").is_some(),
            created_at: row.time("created_at"),
        });
    }
    for row in dump.rows("support_tickets") {
        let (Some(old_id), Some(user)) = (row.string("ticket_id"), row.string("user_id")) else {
            continue;
        };
        let mut list = messages.remove(&old_id).unwrap_or_default();
        list.sort_by_key(|m| m.created_at);
        legacy.tickets.push(LTicket {
            client_old_id: user,
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

fn partner_money(dump: &Dump, legacy: &mut Legacy, owners: &HashMap<String, String>) {
    let mut sums: BTreeMap<String, i64> = BTreeMap::new();
    for row in dump.rows("partner_ledger_entries") {
        if row
            .text("state")
            .is_some_and(|s| !s.eq_ignore_ascii_case("posted"))
        {
            continue;
        }
        let (Some(partner), Some(amount)) = (row.string("partner_id"), row.i64("amount_minor"))
        else {
            continue;
        };
        *sums.entry(partner).or_default() += scaled(amount, row.i64("currency_scale").unwrap_or(2));
    }
    let mut holders = 0u64;
    let mut total = 0i64;
    for (partner, amount) in &sums {
        if *amount <= 0 {
            continue;
        }
        holders += 1;
        total += amount;
        legacy.problem(
            "partner_balance",
            "client",
            owners.get(partner).cloned(),
            None,
            format!(
                "На партнёрском балансе {} — не перенесено, начислите вручную",
                super::stealthnet::format_minor(*amount)
            ),
        );
    }
    legacy.skip(
        "partner_balance",
        holders,
        format!(
            "партнёров с деньгами на партнёрском балансе (всего {}): такой баланс не переносим. Начислите его вручную — список в отчёте.",
            super::stealthnet::format_minor(total)
        ),
    );
    let pending: Vec<(Option<String>, i64)> = dump
        .rows("partner_withdrawals")
        .filter(|r| {
            r.text("status").is_some_and(|s| {
                matches!(s.to_ascii_lowercase().as_str(), "requested" | "processing")
            })
        })
        .map(|r| {
            (
                r.string("partner_id").and_then(|p| owners.get(&p).cloned()),
                scaled(
                    r.i64("debit_amount_minor").unwrap_or(0),
                    r.i64("currency_scale").unwrap_or(2),
                ),
            )
        })
        .collect();
    for (user, amount) in &pending {
        legacy.problem(
            "withdrawal_pending",
            "client",
            user.clone(),
            None,
            format!(
                "Заявка на вывод {} не решена в старом боте",
                super::stealthnet::format_minor(*amount)
            ),
        );
    }
    legacy.skip(
        "withdrawal_pending",
        pending.len() as u64,
        "заявок на вывод партнёрских денег ждут решения. Решите их в старом боте до переключения.",
    );
}

pub fn map(dump: &Dump) -> Legacy {
    let config: Value = dump
        .files
        .get(TARIFFS_FILE)
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(Value::Null);
    let settings = overrides(dump);
    let mut legacy = Legacy::new(Source::Minishop, decide_currency(dump, &config, &settings));
    legacy.dumped_at = dump
        .files
        .get(MANIFEST_FILE)
        .and_then(|m| serde_json::from_str::<Value>(m).ok())
        .and_then(|m| {
            m.get("created_at")
                .and_then(Value::as_str)
                .and_then(parse_time)
        })
        .or(dump.dumped_at);
    let tariffs = tariffs(&config, &mut legacy);
    let with_sub: HashSet<String> = dump
        .rows("subscriptions")
        .filter_map(|r| r.string("user_id"))
        .collect();
    let (owners, percents) = partners(dump);
    let kept = clients(dump, &mut legacy, &with_sub, &percents);
    let unknown = subscriptions(dump, &mut legacy, &kept, &tariffs);
    tariff_notes(&mut legacy, config.get("tariffs").is_some(), &unknown);
    payments(dump, &mut legacy, &kept, &tariffs);
    promos(dump, &mut legacy, &tariffs);
    tickets(dump, &mut legacy);
    partner_money(dump, &mut legacy, &owners);
    legacy
}
