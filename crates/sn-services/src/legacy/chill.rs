use super::model::*;
use super::table::{Dump, Row, minor_from_float};
use serde::Deserialize;
use sn_domain::client::Locale;
use sn_domain::money::Currency;
use sn_domain::subscription::SubscriptionKind;
use sn_domain::traffic::TrafficReset;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub const TABLES: &[&str] = &[
    "settings",
    "users",
    "web_users",
    "payments",
    "plans",
    "plan_access",
    "promo_codes",
    "promo_redemptions",
    "autopay",
    "p2p_requests",
];

pub const CATEGORY: &str = "chill:main";
const DEFAULT_SUFFIX: &str = "_addsub";
const DAYS_IN_MONTH: u32 = 30;

pub fn looks_like(dump: &Dump) -> bool {
    dump.format == "chill"
        || dump
            .table("users")
            .is_some_and(|t| t.has("telegram_id") && t.has("sub_expire_at"))
}

#[derive(Default)]
struct Conf {
    currency: Option<Currency>,
    addsub: Option<bool>,
    suffix: Option<String>,
    english: bool,
}

#[derive(Default, Deserialize)]
struct Snapshot {
    code: Option<String>,
    months: Option<i64>,
    days: Option<i64>,
    price: Option<String>,
    currency: Option<String>,
    addsub: Option<bool>,
}

enum Amount {
    Money(Option<Currency>, f64),
    Stars(f64),
    Unknown,
}

fn currency_of(raw: &str) -> Option<Currency> {
    match raw.trim().trim_end_matches('.').to_lowercase().as_str() {
        "rub" | "rur" | "₽" | "руб" | "р" => Some(Currency::Rub),
        "usd" | "$" => Some(Currency::Usd),
        _ => None,
    }
}

fn amount(raw: &str) -> Option<Amount> {
    let raw = raw.trim();
    let end = raw
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | ',' | ' ' | '\u{a0}')))
        .unwrap_or(raw.len());
    let number: String = raw[..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == ',' { '.' } else { c })
        .collect();
    let value: f64 = number.parse().ok().filter(|v: &f64| v.is_finite())?;
    let unit = raw[end..].trim();
    Some(if unit.is_empty() {
        Amount::Money(None, value)
    } else if unit.contains('⭐')
        || unit.eq_ignore_ascii_case("xtr")
        || unit.to_lowercase().contains("star")
    {
        Amount::Stars(value)
    } else {
        match currency_of(unit) {
            Some(currency) => Amount::Money(Some(currency), value),
            None => Amount::Unknown,
        }
    })
}

fn snapshot(row: &Row<'_>) -> Snapshot {
    row.text("plan_snapshot")
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default()
}

fn conf(dump: &Dump) -> Conf {
    let Some(value) = dump
        .rows("settings")
        .find_map(|r| r.string("config"))
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    else {
        return Conf::default();
    };
    let text = |pointer: &str| {
        value
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    Conf {
        currency: ["/pricing/currency", "/yookassa/currency", "/p2p/currency"]
            .into_iter()
            .find_map(|p| text(p).and_then(currency_of)),
        addsub: value
            .pointer("/addsub/enabled")
            .and_then(serde_json::Value::as_bool),
        suffix: text("/addsub/username_suffix")
            .filter(|s| {
                s.len() <= 20
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
            .map(str::to_string),
        english: text("/language").is_some_and(|l| l.eq_ignore_ascii_case("en")),
    }
}

fn decide_currency(dump: &Dump, conf: &Conf) -> Currency {
    if let Some(currency) = conf.currency {
        return currency;
    }
    let mut tally: HashMap<Currency, usize> = HashMap::new();
    for currency in dump
        .rows("plans")
        .filter_map(|r| r.text("currency").and_then(currency_of))
    {
        *tally.entry(currency).or_default() += 1;
    }
    if tally.is_empty() {
        for row in dump.rows("payments") {
            if let Some(Amount::Money(Some(currency), _)) = row.text("amount").and_then(amount) {
                *tally.entry(currency).or_default() += 1;
            }
        }
    }
    tally
        .into_iter()
        .max_by_key(|(c, n)| (*n, *c == Currency::Rub))
        .map_or(Currency::Rub, |(c, _)| c)
}

fn minor(value: f64) -> Option<i64> {
    minor_from_float(value).filter(|v| *v >= 0)
}

fn duration_days(item: &serde_json::Value) -> Option<u32> {
    let months = item
        .get("months")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let days = item
        .get("days")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let total = if months > 0 {
        months * u64::from(DAYS_IN_MONTH)
    } else {
        days
    };
    u32::try_from(total).ok().filter(|d| (1..=3650).contains(d))
}

fn tariffs(dump: &Dump, legacy: &mut Legacy, conf: &Conf) -> HashMap<String, bool> {
    let mut plans = HashMap::new();
    for row in dump.rows("plans") {
        let Some(code) = row.string("code") else {
            continue;
        };
        let currency = row
            .text("currency")
            .and_then(currency_of)
            .unwrap_or(legacy.currency);
        let mut notes: Vec<String> = Vec::new();
        let mut periods: Vec<LPeriod> = Vec::new();
        let mut stars = false;
        let mut overrides = false;
        if let Some(serde_json::Value::Array(items)) = row.json("durations") {
            for (index, item) in items.iter().enumerate() {
                let Some(days) = duration_days(item) else {
                    continue;
                };
                if periods.iter().any(|p| p.days == days) {
                    continue;
                }
                stars |= item
                    .get("stars")
                    .and_then(serde_json::Value::as_i64)
                    .is_some_and(|s| s > 0);
                overrides |= ["traffic_gb", "device_limit", "int_squads", "ext_squad"]
                    .iter()
                    .any(|k| item.get(*k).is_some_and(|v| !v.is_null()));
                let price = item
                    .get("base")
                    .and_then(serde_json::Value::as_str)
                    .and_then(amount)
                    .and_then(|a| match a {
                        Amount::Money(_, value) => minor(value),
                        _ => None,
                    });
                periods.push(LPeriod {
                    old_id: None,
                    days,
                    price_minor: price,
                    sort: index as i32,
                });
            }
        }
        periods.sort_by_key(|p| p.days);
        let traffic_gb = row.i64("traffic_gb").unwrap_or(0).max(0);
        let reset = match parse_reset(row.text("strategy")) {
            Some(reset) => reset,
            None if traffic_gb > 0 => {
                notes.push(
                    "сброс трафика в старом боте не указан — стоит «раз в месяц», проверьте".into(),
                );
                TrafficReset::Month
            }
            None => TrafficReset::NoReset,
        };
        let addsub = row.text("addsub").unwrap_or("").to_ascii_lowercase();
        let sells_addsub = match addsub.as_str() {
            "on" => true,
            "off" => false,
            _ => conf.addsub == Some(true),
        };
        if row
            .text("availability")
            .is_some_and(|a| !a.eq_ignore_ascii_case("all"))
        {
            notes.push("в старом боте тариф был доступен не всем (новым, старым, по списку или по ссылке) — здесь он открыт всем, проверьте".into());
        }
        if overrides {
            notes.push("у некоторых сроков в старом боте были свои трафик, устройства или серверы — здесь условия общие для тарифа".into());
        }
        if stars {
            notes.push("цены в звёздах не переносятся — задайте их в тарифе".into());
        }
        if sells_addsub {
            notes.push("в старом боте тариф давал второй аккаунт в панели — он перенесён отдельной подпиской и продлевается отдельно".into());
        }
        if currency != legacy.currency {
            notes.push(format!(
                "цены тарифа в {}, а основная валюта {} — тариф снят с продажи",
                currency.code(),
                legacy.currency.code()
            ));
        }
        let squads: Vec<Uuid> = row
            .list("int_squads")
            .iter()
            .filter_map(|s| Uuid::parse_str(s.trim()).ok())
            .collect();
        legacy.tariffs.push(LTariff {
            old_id: code.clone(),
            category_old_id: CATEGORY.into(),
            name: row.string("name").unwrap_or_else(|| code.clone()),
            description: row.string("description"),
            sort: row.i64("sort_order").unwrap_or(0) as i32,
            on_sale: row.flag("enabled") && currency == legacy.currency,
            currency,
            periods,
            devices_included: row
                .i64("device_limit")
                .and_then(|d| u32::try_from(d).ok())
                .unwrap_or(0),
            extra_devices_max: 0,
            extra_device_monthly_minor: None,
            traffic_limit_bytes: gb_to_bytes(traffic_gb),
            traffic_reset: reset,
            squads,
            external_squad: row.text("ext_squad").and_then(|v| Uuid::parse_str(v).ok()),
            note: (!notes.is_empty()).then(|| notes.join("; ")),
        });
        plans.insert(code, sells_addsub);
    }
    if !legacy.tariffs.is_empty() {
        legacy.categories.push(LCategory {
            old_id: CATEGORY.into(),
            name: "Тарифы".into(),
            sort: 0,
        });
    }
    plans
}

struct Web {
    email: String,
    hash: Option<String>,
    verified: bool,
}

fn web_users(dump: &Dump) -> HashMap<i64, Web> {
    dump.rows("web_users")
        .filter_map(|r| {
            Some((
                r.i64("tg_id")?,
                Web {
                    email: r.string("email")?,
                    hash: r.string("pass_hash"),
                    verified: r.text("email_verified_at").is_some(),
                },
            ))
        })
        .collect()
}

fn clients(dump: &Dump, legacy: &mut Legacy, conf: &Conf) {
    let web = web_users(dump);
    let mut blocked = 0u64;
    for row in dump.rows("users") {
        let Some(id) = row.i64("telegram_id") else {
            continue;
        };
        let telegram_id = (id > 0).then_some(id);
        let account = web.get(&id);
        let email = account.and_then(|w| clean_email(Some(&w.email)));
        let client = LClient {
            old_id: id.to_string(),
            telegram_id,
            username: clean_username(row.text("username")),
            first_name: row.string("first_name"),
            last_name: None,
            email_raw: email.as_ref().and(account.map(|w| w.email.clone())),
            email,
            email_verified: account.is_some_and(|w| w.verified),
            password_hash: account.and_then(|w| password_hash(w.hash.as_deref())),
            google_id: None,
            apple_id: None,
            locale: if conf.english { Locale::En } else { Locale::Ru },
            blocked: row.flag("blocked"),
            block_reason: row
                .flag("blocked")
                .then(|| "Заблокирован в старом боте".to_string()),
            bot_unreachable: row.text("unreachable_at").is_some(),
            balance_minor: row.i64("balance").unwrap_or(0).max(0),
            balance_mixed: false,
            referral_code: telegram_id.map(|tg| tg.to_string()),
            referrer_old_id: row
                .i64("referred_by")
                .filter(|r| *r != 0 && *r != id)
                .map(|r| r.to_string()),
            referral_percent: None,
            utm: None,
            trial_used: row.text("trial_used_at").is_some() || row.text("sub_expire_at").is_some(),
            created_at: row.time("created_at"),
        };
        if client.blocked {
            blocked += 1;
            legacy.problem(
                "blocked_flag",
                "client",
                Some(client.old_id.clone()),
                Some(client.who()),
                "В старом боте заблокирован — перенесён заблокированным",
            );
        }
        legacy.clients.push(client);
        bad_email(legacy, account.map(|w| w.email.as_str()));
    }
    legacy.skip(
        "users_blocked",
        blocked,
        "клиентов были заблокированы в старом боте — перенесены заблокированными, список в отчёте.",
    );
}

fn paid_users(dump: &Dump) -> HashSet<i64> {
    dump.rows("payments")
        .filter(|r| {
            r.text("status")
                .is_some_and(|s| s.eq_ignore_ascii_case("paid"))
                && !matches!(
                    r.text("method").map(str::to_ascii_lowercase).as_deref(),
                    Some("trial" | "grant")
                )
        })
        .filter_map(|r| r.i64("telegram_id"))
        .collect()
}

fn subscriptions(dump: &Dump, legacy: &mut Legacy, conf: &Conf, plans: &HashMap<String, bool>) {
    let paid = paid_users(dump);
    let suffix = conf.suffix.as_deref().unwrap_or(DEFAULT_SUFFIX);
    let mut extra = 0u64;
    for row in dump.rows("users") {
        let Some(id) = row.i64("telegram_id") else {
            continue;
        };
        let expire_at = row.time("sub_expire_at");
        let trial = row.text("trial_used_at").is_some();
        let snap = snapshot(&row);
        if expire_at.is_none() && !trial && row.text("plan_snapshot").is_none() {
            continue;
        }
        let plan = snap
            .code
            .as_ref()
            .filter(|c| plans.contains_key(*c))
            .cloned();
        let kind = if paid.contains(&id) || !trial {
            SubscriptionKind::Paid
        } else {
            SubscriptionKind::Trial
        };
        let base = LSubscription {
            old_id: format!("tg_{id}"),
            client_old_id: id.to_string(),
            tariff_old_id: plan.clone(),
            kind,
            remna_id: None,
            remna_uuid: None,
            short_uuid: None,
            expire_at,
            extra_devices: 0,
            autorenew: false,
            autorenew_days: None,
            autorenew_period_old_id: None,
            ordinal: 0,
            gifted: false,
            created_at: row.time("created_at"),
        };
        let second = plan.as_ref().and_then(|p| plans.get(p)).copied() == Some(true)
            && snap.addsub != Some(false);
        let addsub = second.then(|| LSubscription {
            old_id: format!("tg_{id}{suffix}"),
            ordinal: 1,
            ..base.clone()
        });
        legacy.subscriptions.push(base);
        if let Some(addsub) = addsub {
            extra += 1;
            legacy.subscriptions.push(addsub);
        }
    }
    if plans.is_empty() {
        let count = legacy.subscriptions.len() as u64;
        legacy.skip(
            "tariffs_manual",
            count,
            "подписок перейдут без тарифа: тарифов в копии нет. Заведите тарифы вручную в «Тарифах» — при продлении клиент выберет тариф.",
        );
    }
    legacy.skip(
        "addsub_split",
        extra,
        "вторых аккаунтов в панели (доп-подписка тарифа) перенесены отдельными подписками: в новой системе каждая продлевается сама.",
    );
}

fn provider_of(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "cryptobot" | "crypto_bot" => "cryptopay".into(),
        "p2p" => "manual".into(),
        "" => "unknown".into(),
        other => other.to_string(),
    }
}

fn payments(dump: &Dump, legacy: &mut Legacy) {
    let mut unpaid = 0u64;
    let mut free = 0u64;
    let mut unknown = 0u64;
    for row in dump.rows("payments") {
        let (Some(old_id), Some(user)) = (row.string("id"), row.i64("telegram_id")) else {
            continue;
        };
        let status = row.text("status").unwrap_or("").to_ascii_lowercase();
        let refunded = match status.as_str() {
            "paid" => false,
            "refunded" => true,
            _ => {
                unpaid += 1;
                continue;
            }
        };
        let method = row.text("method").unwrap_or("").to_ascii_lowercase();
        if matches!(method.as_str(), "trial" | "grant") {
            free += 1;
            continue;
        }
        let snap = snapshot(&row);
        let snap_money = || {
            let value = snap.price.as_deref().and_then(amount)?;
            match value {
                Amount::Money(currency, value) => Some((
                    currency.or_else(|| snap.currency.as_deref().and_then(currency_of)),
                    value,
                )),
                _ => None,
            }
        };
        let (currency, amount_minor) = match row.text("amount").and_then(amount) {
            Some(Amount::Stars(value)) => (PayCurrencyLegacy::Stars, value.round() as i64),
            Some(Amount::Money(currency, value)) => (
                PayCurrencyLegacy::Money(
                    currency
                        .or_else(|| snap.currency.as_deref().and_then(currency_of))
                        .unwrap_or(legacy.currency),
                ),
                minor(value).unwrap_or(0),
            ),
            Some(Amount::Unknown) | None => match snap_money() {
                Some((currency, value)) => (
                    PayCurrencyLegacy::Money(currency.unwrap_or(legacy.currency)),
                    minor(value).unwrap_or(0),
                ),
                None if row.text("amount").is_some() => {
                    unknown += 1;
                    continue;
                }
                None => (PayCurrencyLegacy::Money(legacy.currency), 0),
            },
        };
        if amount_minor <= 0 {
            free += 1;
            continue;
        }
        let months = row
            .i64("months")
            .and_then(|m| u32::try_from(m).ok())
            .filter(|m| *m > 0);
        let days = months
            .map(|m| m * DAYS_IN_MONTH)
            .or_else(|| {
                snap.days
                    .and_then(|d| u32::try_from(d).ok())
                    .filter(|d| *d > 0)
            })
            .or_else(|| {
                snap.months
                    .and_then(|m| u32::try_from(m).ok())
                    .filter(|m| *m > 0)
                    .map(|m| m * DAYS_IN_MONTH)
            });
        let topup = months.is_none()
            && row
                .text("comment")
                .is_some_and(|c| c.eq_ignore_ascii_case("topup"));
        let kind = if topup {
            PaymentKind::TopUp
        } else if months.is_some() {
            PaymentKind::Subscription
        } else {
            PaymentKind::Other
        };
        legacy.payments.push(LPayment {
            client_old_id: user.to_string(),
            kind,
            refunded,
            amount_minor,
            currency,
            provider: provider_of(&method),
            external_id: row.string("ext_id"),
            tariff_old_id: snap
                .code
                .clone()
                .filter(|_| kind == PaymentKind::Subscription),
            days: days.filter(|_| kind == PaymentKind::Subscription),
            subscription_old_id: None,
            label: (kind == PaymentKind::Subscription && method == "balance")
                .then(|| "Покупка подписки с баланса".to_string()),
            created_at: row.time("created_at"),
            paid_at: row.time("created_at"),
            old_id,
        });
    }
    legacy.skip(
        "payments_unpaid",
        unpaid,
        "неоплаченных и отклонённых платежей: в истории они не нужны.",
    );
    legacy.skip(
        "payments_free",
        free,
        "выдач без оплаты (пробный период, подарок от админа): в истории платежей их нет, подписки переносятся.",
    );
    legacy.skip(
        "payments_currency",
        unknown,
        "оплат в криптовалюте без суммы в рублях или долларах: их нет в истории.",
    );
}

fn promo_code(raw: &str) -> Option<String> {
    let code = raw.trim().to_ascii_uppercase();
    ((3..=32).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then_some(code)
}

fn promos(dump: &Dump, legacy: &mut Legacy) {
    let mut unsupported = 0u64;
    for row in dump.rows("promo_codes") {
        let Some(raw) = row.string("code") else {
            continue;
        };
        let kind = row.text("kind").unwrap_or("").to_ascii_lowercase();
        let value = row.i64("value").unwrap_or(0);
        let code = promo_code(&raw);
        let days = (kind == "days")
            .then(|| u32::try_from(value).ok().filter(|d| (1..=3650).contains(d)))
            .flatten();
        let (Some(code), Some(days)) = (code.clone(), days) else {
            unsupported += 1;
            legacy.problem(
                "promo_unsupported",
                "promo",
                Some(raw.clone()),
                Some(raw),
                match kind.as_str() {
                    _ if code.is_none() => {
                        "Код промокода должен быть из латинских букв и цифр, от 3 до 32 знаков — не перенесён"
                    }
                    "balance" => "Промокод пополнял баланс — в новой системе промокоды дают скидку или дни",
                    "traffic" | "traffic_period" => {
                        "Промокод давал трафик — в новой системе промокоды дают скидку или дни"
                    }
                    _ => "Промокод без дней — не перенесён",
                },
            );
            continue;
        };
        legacy.promos.push(LPromo {
            old_id: raw,
            code,
            value: PromoValue::Days(days),
            max_uses: u32::try_from(row.i64("max_uses").unwrap_or(0))
                .ok()
                .filter(|v| *v > 0),
            per_client: 1,
            valid_from: None,
            valid_until: row.time("expires_at"),
            active: true,
            first_purchase_only: false,
            tariff_old_id: None,
            note: None,
        });
    }
    for row in dump.rows("promo_redemptions") {
        let (Some(code), Some(user)) = (row.string("code"), row.i64("telegram_id")) else {
            continue;
        };
        legacy.promo_uses.push(LPromoUse {
            old_id: format!("{code}:{user}"),
            promo_old_id: code,
            client_old_id: user.to_string(),
            used_at: row.time("created_at"),
        });
    }
    legacy.skip(
        "promo_unsupported",
        unsupported,
        "промокодов другого вида (на баланс или трафик): в новой системе промокоды дают скидку или дни. Список в отчёте.",
    );
}

fn leftovers(dump: &Dump, legacy: &mut Legacy) {
    let autopay = dump.rows("autopay").filter(|r| r.flag("enabled")).count() as u64;
    legacy.skip(
        "autopay_cards",
        autopay,
        "автоплатежей с сохранённой карты: карты остались у старого бота. Клиентам нужно заново включить автопродление.",
    );
    let waiting = dump
        .rows("p2p_requests")
        .filter(|r| {
            r.text("status").is_some_and(|s| {
                s.eq_ignore_ascii_case("awaiting") || s.eq_ignore_ascii_case("submitted")
            })
        })
        .count() as u64;
    legacy.skip(
        "p2p_waiting",
        waiting,
        "заявок на оплату переводом ждут проверки. Решите их в старом боте до переключения.",
    );
    legacy.skip(
        "plan_access",
        dump.count("plan_access") as u64,
        "доступов к закрытым тарифам по списку: здесь тарифы открыты всем, проверьте тарифы с пометкой в отчёте.",
    );
}

pub fn map(dump: &Dump) -> Legacy {
    let conf = conf(dump);
    let mut legacy = Legacy::new(Source::Chill, decide_currency(dump, &conf));
    legacy.dumped_at = dump.dumped_at;
    let plans = tariffs(dump, &mut legacy, &conf);
    clients(dump, &mut legacy, &conf);
    subscriptions(dump, &mut legacy, &conf, &plans);
    payments(dump, &mut legacy);
    promos(dump, &mut legacy);
    leftovers(dump, &mut legacy);
    legacy
}
