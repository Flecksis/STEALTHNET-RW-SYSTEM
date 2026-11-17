use super::model::*;
use super::table::{Dump, Row};
use sn_domain::money::Currency;
use sn_domain::subscription::SubscriptionKind;
use sn_domain::traffic::TrafficReset;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use uuid::Uuid;

pub const TABLES: &[&str] = &[
    "users",
    "subscriptions",
    "transactions",
    "tariffs",
    "server_squads",
    "promocodes",
    "promocode_uses",
    "promo_groups",
    "tickets",
    "ticket_messages",
    "system_settings",
    "withdrawal_requests",
];

pub const CLASSIC_CATEGORY: &str = "bedolaga:classic";
pub const MAIN_CATEGORY: &str = "bedolaga:main";

pub fn looks_like(dump: &Dump) -> bool {
    dump.table("users")
        .is_some_and(|t| t.has("balance_kopeks") || t.has("telegram_id"))
        && (dump.table("transactions").is_some() || dump.table("subscriptions").is_some())
}

fn settings(dump: &Dump) -> HashMap<String, String> {
    dump.rows("system_settings")
        .filter_map(|r| {
            Some((
                r.string("key")?.to_ascii_uppercase(),
                r.raw("value")
                    .unwrap_or("")
                    .trim()
                    .trim_matches('"')
                    .to_string(),
            ))
        })
        .collect()
}

fn provider_of(raw: Option<&str>) -> String {
    match raw.map(|r| r.trim().to_ascii_lowercase()).as_deref() {
        Some("cryptobot" | "crypto_bot" | "cryptopay") => "cryptopay".into(),
        Some("telegram_stars" | "stars") => "stars".into(),
        Some("") | None => "unknown".into(),
        Some(other) => other.to_string(),
    }
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

fn u32_of(row: &Row<'_>, column: &str) -> Option<u32> {
    row.i64(column).and_then(|v| u32::try_from(v).ok())
}

fn traffic_label(gb: i64) -> String {
    if gb <= 0 {
        "без лимита трафика".into()
    } else {
        format!("{gb} ГБ")
    }
}

fn clients(
    dump: &Dump,
    legacy: &mut Legacy,
    with_subscription: &HashSet<String>,
) -> HashSet<String> {
    let mut deleted = 0u64;
    let mut flagged = 0u64;
    let mut kept = HashSet::new();
    let mut seen_tg = HashSet::new();
    for row in dump.rows("users") {
        let Some(old_id) = row.string("id") else {
            continue;
        };
        let status = row.text("status").unwrap_or("active").to_ascii_lowercase();
        if status == "deleted" {
            deleted += 1;
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
        let blocked = status == "blocked";
        if blocked {
            flagged += 1;
        }
        let client = LClient {
            telegram_id,
            username: clean_username(row.text("username")),
            first_name: row.string("first_name"),
            last_name: row.string("last_name"),
            email: clean_email(row.text("email")),
            email_verified: row.flag("email_verified"),
            password_hash: password_hash(row.text("password_hash")),
            google_id: row.string("google_id"),
            apple_id: None,
            locale: locale_of(row.text("language")),
            blocked: false,
            block_reason: None,
            bot_unreachable: blocked,
            balance_minor: row.i64("balance_kopeks").unwrap_or(0).max(0),
            balance_mixed: false,
            referral_code: row.string("referral_code"),
            referrer_old_id: row.string("referred_by_id"),
            referral_percent: row
                .i64("referral_commission_percent")
                .filter(|p| (0..=100).contains(p))
                .map(|p| p as i32),
            utm: None,
            trial_used: row.flag("has_had_paid_subscription")
                || with_subscription.contains(&old_id),
            created_at: row.time("created_at"),
            old_id: old_id.clone(),
        };
        if blocked {
            legacy.problem(
                "blocked_flag",
                "client",
                Some(old_id.clone()),
                Some(client.who()),
                "В старом боте помечен заблокированным: так помечаются и те, кто остановил бота. Перенесён без блокировки",
            );
        }
        kept.insert(old_id);
        legacy.clients.push(client);
    }
    legacy.skip(
        "users_deleted",
        deleted,
        "удалённых пользователей: их уже нет в старом боте.",
    );
    legacy.skip(
        "users_blocked",
        flagged,
        "пользователей старый бот пометил заблокированными — так он помечает и тех, кто остановил бота. Перенесены без блокировки, список в отчёте.",
    );
    kept
}

struct Shape {
    squads: Vec<Uuid>,
    traffic_gb: i64,
}

fn tariffs(
    dump: &Dump,
    legacy: &mut Legacy,
    settings: &HashMap<String, String>,
) -> HashMap<String, u32> {
    let default_reset = settings
        .get("DEFAULT_TRAFFIC_RESET_STRATEGY")
        .and_then(|v| parse_reset(Some(v)));
    let mut devices = HashMap::new();
    let mut created = false;
    for row in dump.rows("tariffs") {
        let (Some(old_id), Some(name)) = (row.string("id"), row.string("name")) else {
            continue;
        };
        let mut periods: Vec<LPeriod> = match row.json("period_prices") {
            Some(serde_json::Value::Object(map)) => map
                .iter()
                .filter_map(|(days, price)| {
                    let days: u32 = days.trim().parse().ok()?;
                    let price = price
                        .as_i64()
                        .or_else(|| price.as_str().and_then(|s| s.trim().parse().ok()))
                        .or_else(|| price.as_f64().map(|f| f.round() as i64))?;
                    (days > 0 && price >= 0).then_some(LPeriod {
                        old_id: None,
                        days,
                        price_minor: Some(price),
                        sort: days as i32,
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        let mut notes = Vec::new();
        if periods.is_empty()
            && row.flag("is_daily")
            && let Some(price) = row.i64("daily_price_kopeks").filter(|p| *p > 0)
        {
            periods.push(LPeriod {
                old_id: None,
                days: 1,
                price_minor: Some(price),
                sort: 1,
            });
            notes.push(
                "суточного списания в новой системе нет — тариф продаётся на 1 день".to_string(),
            );
        }
        periods.sort_by_key(|p| p.days);
        let traffic_gb = row.i64("traffic_limit_gb").unwrap_or(0);
        let reset = match parse_reset(row.text("traffic_reset_mode")).or(default_reset) {
            Some(reset) => reset,
            None if traffic_gb > 0 => {
                notes.push("сброс трафика брался из общих настроек старого бота — стоит «раз в месяц», проверьте".to_string());
                TrafficReset::Month
            }
            None => TrafficReset::NoReset,
        };
        let included = u32_of(&row, "device_limit").unwrap_or(1).max(1);
        let device_price = row.i64("device_price_kopeks").filter(|p| *p > 0);
        let max_total = u32_of(&row, "max_device_limit");
        let extra_max = match (device_price, max_total) {
            (Some(_), Some(max)) => max.saturating_sub(included),
            (Some(_), None) => 10,
            _ => 0,
        };
        devices.insert(old_id.clone(), included);
        created = true;
        let on_sale = row.bool("is_active").unwrap_or(true) && !periods.is_empty();
        legacy.tariffs.push(LTariff {
            category_old_id: MAIN_CATEGORY.into(),
            name,
            description: row.string("description"),
            sort: row.i64("display_order").unwrap_or(0) as i32,
            on_sale,
            currency: Currency::Rub,
            periods,
            devices_included: included,
            extra_devices_max: extra_max,
            extra_device_monthly_minor: device_price.filter(|_| extra_max > 0),
            traffic_limit_bytes: gb_to_bytes(traffic_gb),
            traffic_reset: reset,
            squads: uuids(row.list("allowed_squads")),
            external_squad: row
                .text("external_squad_uuid")
                .and_then(|v| Uuid::parse_str(v).ok()),
            note: (!notes.is_empty()).then(|| notes.join("; ")),
            old_id,
        });
    }
    if created {
        legacy.categories.push(LCategory {
            old_id: MAIN_CATEGORY.into(),
            name: "Тарифы".into(),
            sort: 0,
        });
    }
    devices
}

fn classic_tariffs(
    dump: &Dump,
    legacy: &mut Legacy,
    settings: &HashMap<String, String>,
    shapes: &BTreeMap<(Vec<Uuid>, i64), (u32, u32)>,
) -> HashMap<(Vec<Uuid>, i64), String> {
    let names: HashMap<Uuid, String> = dump
        .rows("server_squads")
        .filter_map(|r| {
            Some((
                Uuid::parse_str(r.text("squad_uuid")?).ok()?,
                r.string("display_name")
                    .or_else(|| r.string("original_name"))?,
            ))
        })
        .collect();
    let periods: Vec<u32> = settings
        .get("AVAILABLE_SUBSCRIPTION_PERIODS")
        .map(|v| {
            v.split(',')
                .filter_map(|p| p.trim().parse::<u32>().ok())
                .filter(|d| *d > 0)
                .collect::<BTreeSet<u32>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default();
    let device_price = settings
        .get("PRICE_PER_DEVICE")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0);
    let reset = settings
        .get("DEFAULT_TRAFFIC_RESET_STRATEGY")
        .and_then(|v| parse_reset(Some(v)));
    let mut out = HashMap::new();
    for (index, ((squads, traffic_gb), (min_devices, max_devices))) in shapes.iter().enumerate() {
        let servers = match squads.len() {
            0 => "Без серверов".to_string(),
            1..=3 => squads
                .iter()
                .map(|s| {
                    names
                        .get(s)
                        .cloned()
                        .unwrap_or_else(|| s.to_string()[..8].to_string())
                })
                .collect::<Vec<_>>()
                .join(", "),
            n => format!("{n} серверов"),
        };
        let old_id = format!("classic:{index}");
        let extra = max_devices.saturating_sub(*min_devices);
        legacy.tariffs.push(LTariff {
            old_id: old_id.clone(),
            category_old_id: CLASSIC_CATEGORY.into(),
            name: format!("{servers} · {}", traffic_label(*traffic_gb)),
            description: None,
            sort: index as i32,
            on_sale: false,
            currency: Currency::Rub,
            periods: periods
                .iter()
                .map(|days| LPeriod {
                    old_id: None,
                    days: *days,
                    price_minor: None,
                    sort: *days as i32,
                })
                .collect(),
            devices_included: (*min_devices).max(1),
            extra_devices_max: if extra > 0 { extra } else { 0 },
            extra_device_monthly_minor: device_price.filter(|_| extra > 0),
            traffic_limit_bytes: gb_to_bytes(*traffic_gb),
            traffic_reset: if *traffic_gb > 0 {
                reset.unwrap_or(TrafficReset::Month)
            } else {
                TrafficReset::NoReset
            },
            squads: squads.clone(),
            external_squad: None,
            note: Some(
                "в старом боте цена складывалась из срока, трафика, серверов и устройств — задайте цены сами".into(),
            ),
        });
        out.insert((squads.clone(), *traffic_gb), old_id);
    }
    if !shapes.is_empty() {
        legacy.categories.push(LCategory {
            old_id: CLASSIC_CATEGORY.into(),
            name: "Подписки без тарифа".into(),
            sort: 1,
        });
    }
    out
}

fn subscriptions(
    dump: &Dump,
    legacy: &mut Legacy,
    settings: &HashMap<String, String>,
    tariff_devices: &HashMap<String, u32>,
    clients: &HashSet<String>,
) {
    let user_remna: HashMap<String, (Option<i64>, Option<String>)> = dump
        .rows("users")
        .filter_map(|r| {
            Some((
                r.string("id")?,
                (r.i64("remnawave_id"), r.string("remnawave_uuid")),
            ))
        })
        .collect();
    let per_user: HashMap<String, usize> = dump
        .rows("subscriptions")
        .filter_map(|r| r.string("user_id"))
        .fold(HashMap::new(), |mut acc, u| {
            *acc.entry(u).or_default() += 1;
            acc
        });
    let mut pending = 0u64;
    let mut shapes: BTreeMap<(Vec<Uuid>, i64), (u32, u32)> = BTreeMap::new();
    let mut rows: Vec<(LSubscription, Shape, u32)> = Vec::new();
    let mut ordinals: HashMap<String, u32> = HashMap::new();
    for row in dump.rows("subscriptions") {
        let (Some(old_id), Some(user)) = (row.string("id"), row.string("user_id")) else {
            continue;
        };
        if !clients.contains(&user) {
            continue;
        }
        let status = row.text("status").unwrap_or("").to_ascii_lowercase();
        if status == "pending" {
            pending += 1;
            continue;
        }
        let only = per_user.get(&user).copied() == Some(1);
        let fallback = only.then(|| user_remna.get(&user).cloned()).flatten();
        let remna_id = row
            .i64("remnawave_id")
            .or_else(|| fallback.as_ref().and_then(|f| f.0));
        let remna_uuid = row
            .string("remnawave_uuid")
            .or_else(|| fallback.as_ref().and_then(|f| f.1.clone()));
        let short_uuid = row
            .string("remnawave_short_uuid")
            .or_else(|| short_uuid_from_url(row.text("subscription_url")));
        let device_limit = u32_of(&row, "device_limit").unwrap_or(1).max(1);
        let tariff = row.string("tariff_id");
        let extra = tariff
            .as_ref()
            .and_then(|t| tariff_devices.get(t))
            .map(|included| device_limit.saturating_sub(*included))
            .unwrap_or(0);
        let shape = Shape {
            squads: uuids(row.list("connected_squads")),
            traffic_gb: row.i64("traffic_limit_gb").unwrap_or(0).max(0),
        };
        if tariff.is_none() {
            let entry = shapes
                .entry((shape.squads.clone(), shape.traffic_gb))
                .or_insert((device_limit, device_limit));
            entry.0 = entry.0.min(device_limit);
            entry.1 = entry.1.max(device_limit);
        }
        let ordinal = ordinals.entry(user.clone()).or_default();
        *ordinal += 1;
        rows.push((
            LSubscription {
                client_old_id: user,
                tariff_old_id: tariff,
                kind: if row.flag("is_trial") || status == "trial" {
                    SubscriptionKind::Trial
                } else {
                    SubscriptionKind::Paid
                },
                remna_id,
                remna_uuid: remna_uuid.filter(|_| remna_id.is_none()),
                short_uuid,
                expire_at: row.time("end_date"),
                extra_devices: extra,
                autorenew: row.flag("autopay_enabled"),
                autorenew_days: u32_of(&row, "autopay_period_days"),
                autorenew_period_old_id: None,
                ordinal: *ordinal - 1,
                created_at: row.time("created_at").or_else(|| row.time("start_date")),
                old_id,
            },
            shape,
            device_limit,
        ));
    }
    let classic = classic_tariffs(dump, legacy, settings, &shapes);
    if !classic.is_empty() {
        legacy.tariff_mode = TariffMode::Classic;
    }
    for (mut sub, shape, device_limit) in rows {
        if sub.tariff_old_id.is_none()
            && let Some(id) = classic.get(&(shape.squads.clone(), shape.traffic_gb))
        {
            let included = shapes
                .get(&(shape.squads, shape.traffic_gb))
                .map_or(1, |(min, _)| (*min).max(1));
            sub.extra_devices = device_limit.saturating_sub(included);
            sub.tariff_old_id = Some(id.clone());
        }
        legacy.subscriptions.push(sub);
    }
    legacy.skip(
        "subscriptions_pending",
        pending,
        "подписок так и не выдали в старом боте: переносить нечего.",
    );
}

fn payments(dump: &Dump, legacy: &mut Legacy) {
    let mut unpaid = 0u64;
    let mut internal = 0u64;
    for row in dump.rows("transactions") {
        let (Some(old_id), Some(user)) = (row.string("id"), row.string("user_id")) else {
            continue;
        };
        let kind = row.text("type").unwrap_or("").to_ascii_lowercase();
        if !row.bool("is_completed").unwrap_or(true) {
            unpaid += 1;
            continue;
        }
        let amount = row.i64("amount_kopeks").unwrap_or(0).abs();
        let (payment_kind, provider, label) = match kind.as_str() {
            "deposit" => (
                PaymentKind::TopUp,
                provider_of(row.text("payment_method")),
                None,
            ),
            "subscription_payment" => (
                PaymentKind::Subscription,
                "balance".to_string(),
                row.string("description")
                    .map(|d| d.chars().take(120).collect::<String>()),
            ),
            "gift_payment" => (
                PaymentKind::Other,
                "balance".to_string(),
                Some(
                    row.string("description")
                        .map(|d| d.chars().take(120).collect::<String>())
                        .unwrap_or_else(|| "Подарок".into()),
                ),
            ),
            _ => {
                internal += 1;
                continue;
            }
        };
        if amount == 0 {
            internal += 1;
            continue;
        }
        legacy.payments.push(LPayment {
            client_old_id: user,
            kind: payment_kind,
            refunded: false,
            amount_minor: amount,
            currency: PayCurrencyLegacy::Money(Currency::Rub),
            provider,
            external_id: row.string("external_id"),
            tariff_old_id: None,
            days: None,
            subscription_old_id: None,
            label,
            created_at: row.time("created_at"),
            paid_at: row.time("completed_at").or_else(|| row.time("created_at")),
            old_id,
        });
    }
    legacy.skip(
        "payments_unpaid",
        unpaid,
        "незавершённых платежей: в истории они не нужны.",
    );
    legacy.skip(
        "balance_moves",
        internal,
        "движений баланса без оплаты (реферальные бонусы, возвраты, выводы): они уже учтены в балансе клиентов.",
    );
}

fn promos(dump: &Dump, legacy: &mut Legacy) {
    let mut unsupported = 0u64;
    for row in dump.rows("promocodes") {
        let (Some(old_id), Some(raw)) = (row.string("id"), row.string("code")) else {
            continue;
        };
        let code = raw.trim().to_ascii_uppercase();
        let valid = (3..=32).contains(&code.len())
            && code
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        let kind = row.text("type").unwrap_or("").to_ascii_lowercase();
        let days = u32_of(&row, "subscription_days").filter(|d| (1..=3650).contains(d));
        let value = match kind.as_str() {
            "subscription_days" | "balance_and_days" | "trial_subscription" => {
                days.map(PromoValue::Days)
            }
            "discount" => row
                .i64("balance_bonus_kopeks")
                .filter(|p| (1..=100).contains(p))
                .map(|p| PromoValue::Percent(p as u8)),
            _ => None,
        };
        let Some(value) = value.filter(|_| valid) else {
            unsupported += 1;
            legacy.problem(
                "promo_unsupported",
                "promo",
                Some(old_id),
                Some(code),
                match kind.as_str() {
                    "balance" => {
                        "Промокод пополнял баланс — в новой системе промокоды дают скидку или дни"
                    }
                    "promo_group" => "Промокод включал промогруппу — в новой системе таких нет",
                    _ if !valid => {
                        "Код промокода должен быть из латинских букв и цифр, от 3 до 32 знаков"
                    }
                    _ => "Промокод без скидки и без дней",
                },
            );
            continue;
        };
        let note = (kind == "balance_and_days")
            .then(|| "бонус на баланс из старого промокода не переносится".to_string());
        legacy.promos.push(LPromo {
            code,
            value,
            max_uses: u32_of(&row, "max_uses").filter(|v| *v > 0),
            per_client: 1,
            valid_from: row.time("valid_from"),
            valid_until: row.time("valid_until"),
            active: row.bool("is_active").unwrap_or(true),
            first_purchase_only: row.flag("first_purchase_only"),
            tariff_old_id: row.string("tariff_id"),
            note,
            old_id,
        });
    }
    for row in dump.rows("promocode_uses") {
        let (Some(old_id), Some(promo), Some(user)) = (
            row.string("id"),
            row.string("promocode_id"),
            row.string("user_id"),
        ) else {
            continue;
        };
        legacy.promo_uses.push(LPromoUse {
            old_id,
            promo_old_id: promo,
            client_old_id: user,
            used_at: row.time("used_at"),
        });
    }
    legacy.skip(
        "promo_unsupported",
        unsupported,
        "промокодов другого вида (на баланс, на промогруппу): в новой системе промокоды дают скидку или дни. Список в отчёте.",
    );
    legacy.skip(
        "promo_groups",
        dump.count("promo_groups") as u64,
        "промогрупп со скидками: в новой системе их нет, скидки задаются промокодами.",
    );
}

fn tickets(dump: &Dump, legacy: &mut Legacy) {
    let mut messages: HashMap<String, Vec<LMessage>> = HashMap::new();
    for row in dump.rows("ticket_messages") {
        let (Some(old_id), Some(ticket)) = (row.string("id"), row.string("ticket_id")) else {
            continue;
        };
        let text = row.raw("message_text").unwrap_or("").trim().to_string();
        let media = row.flag("has_media");
        let body = match (text.is_empty(), media) {
            (true, false) => continue,
            (true, true) => "[вложение осталось в старом боте]".to_string(),
            (false, true) => format!("{text}\n[вложение осталось в старом боте]"),
            (false, false) => text,
        };
        messages.entry(ticket).or_default().push(LMessage {
            old_id,
            from_support: row.flag("is_from_admin"),
            body,
            read: true,
            created_at: row.time("created_at"),
        });
    }
    for row in dump.rows("tickets") {
        let (Some(old_id), Some(user)) = (row.string("id"), row.string("user_id")) else {
            continue;
        };
        let mut list = messages.remove(&old_id).unwrap_or_default();
        list.sort_by_key(|m| m.created_at);
        legacy.tickets.push(LTicket {
            client_old_id: user,
            subject: row.string("title"),
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

fn withdrawals(dump: &Dump, legacy: &mut Legacy) {
    let pending: Vec<(String, i64)> = dump
        .rows("withdrawal_requests")
        .filter(|r| {
            r.text("status")
                .is_some_and(|s| s.eq_ignore_ascii_case("pending"))
        })
        .filter_map(|r| Some((r.string("user_id")?, r.i64("amount_kopeks").unwrap_or(0))))
        .collect();
    for (user, amount) in &pending {
        legacy.problem(
            "withdrawal_pending",
            "client",
            Some(user.clone()),
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
        "заявок на вывод ждут решения. Решите их в старом боте до переключения.",
    );
}

pub fn map(dump: &Dump) -> Legacy {
    let settings = settings(dump);
    let mut legacy = Legacy::new(Source::Bedolaga, Currency::Rub);
    legacy.dumped_at = dump.dumped_at;
    let with_subscription: HashSet<String> = dump
        .rows("subscriptions")
        .filter_map(|r| r.string("user_id"))
        .collect();
    let devices = tariffs(dump, &mut legacy, &settings);
    let kept = clients(dump, &mut legacy, &with_subscription);
    subscriptions(dump, &mut legacy, &settings, &devices, &kept);
    payments(dump, &mut legacy);
    promos(dump, &mut legacy);
    tickets(dump, &mut legacy);
    withdrawals(dump, &mut legacy);
    legacy
}
