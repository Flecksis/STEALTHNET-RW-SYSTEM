use super::matcher::{Matched, Method, RemnaIndex, match_all};
use super::model::{LClient, Legacy, Source};
use super::runs::{Balances, Counts, CurrencyPlan, Existing, IssueRow, Preview, RemnaPlan};
use crate::Services;
use crate::error::ServiceResult;
use sn_domain::money::Currency;
use sn_remnawave::UsersStreamQuery;
use std::collections::{HashMap, HashSet};

const PAGE: u32 = 500;
const MAX_PAGES: usize = 20_000;

pub async fn fetch_remna(services: &Services) -> Result<Option<RemnaIndex>, String> {
    let Ok(api) = services.remna() else {
        return Ok(None);
    };
    let mut users = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let page = api
            .stream_users(&UsersStreamQuery {
                cursor: cursor.clone(),
                size: Some(PAGE),
                ..UsersStreamQuery::default()
            })
            .await
            .map_err(|err| err.to_string())?;
        let got = page.users.len();
        users.extend(page.users);
        match page.next_cursor {
            Some(next) if page.has_more && got > 0 && Some(&next) != cursor.as_ref() => {
                cursor = Some(next);
            }
            _ => break,
        }
    }
    Ok(Some(RemnaIndex::new(users)))
}

pub async fn links(
    services: &Services,
    user_ids: &[i64],
) -> ServiceResult<HashMap<i64, (i64, i64)>> {
    let mut conn = services.store().conn().await?;
    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT remna_user_id, id, client_id FROM subscriptions WHERE remna_user_id = ANY($1)",
    )
    .bind(user_ids)
    .fetch_all(sn_store::Db::conn(&mut conn))
    .await?;
    Ok(rows.into_iter().map(|(r, s, c)| (r, (s, c))).collect())
}

pub async fn legacy_map(
    services: &Services,
    entity: &str,
) -> ServiceResult<HashMap<String, (i64, serde_json::Value)>> {
    let mut conn = services.store().conn().await?;
    let rows: Vec<(String, i64, serde_json::Value)> =
        sqlx::query_as("SELECT old_id, new_id, data FROM legacy_ids WHERE entity = $1")
            .bind(entity)
            .fetch_all(sn_store::Db::conn(&mut conn))
            .await?;
    Ok(rows.into_iter().map(|(o, n, d)| (o, (n, d))).collect())
}

pub async fn identity_owners(
    services: &Services,
    legacy: &Legacy,
) -> ServiceResult<(HashMap<i64, i64>, HashMap<String, i64>)> {
    let telegram: Vec<String> = legacy
        .clients
        .iter()
        .filter_map(|c| c.telegram_id.map(|t| t.to_string()))
        .collect();
    let emails: Vec<String> = legacy
        .clients
        .iter()
        .filter_map(|c| c.email.clone())
        .collect();
    let mut conn = services.store().conn().await?;
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT i.kind, i.value, i.client_id FROM client_identities i JOIN clients c ON c.id = i.client_id \
         WHERE c.deleted_at IS NULL AND ((i.kind = 'telegram' AND i.value = ANY($1)) OR (i.kind = 'email' AND i.value = ANY($2)))",
    )
    .bind(&telegram)
    .bind(&emails)
    .fetch_all(sn_store::Db::conn(&mut conn))
    .await?;
    let mut by_tg = HashMap::new();
    let mut by_email = HashMap::new();
    for (kind, value, client) in rows {
        if kind == "telegram" {
            if let Ok(tg) = value.parse::<i64>() {
                by_tg.insert(tg, client);
            }
        } else {
            by_email.insert(value, client);
        }
    }
    Ok((by_tg, by_email))
}

async fn currency_plan(services: &Services, source: Currency) -> ServiceResult<CurrencyPlan> {
    let install = services.settings().currency;
    if source == install {
        return Ok(CurrencyPlan {
            source,
            install,
            switch: false,
            blocked: None,
        });
    }
    let mut conn = services.store().conn().await?;
    let busy: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM payments WHERE status IN ('paid', 'refunded', 'partially_refunded') \
             AND NOT (provider_payload ? 'imported')) \
         OR EXISTS (SELECT 1 FROM client_balances WHERE currency = $1 AND amount_minor > 0 AND wallet = 'main')",
    )
    .bind(install.code())
    .fetch_one(sn_store::Db::conn(&mut conn))
    .await?;
    Ok(CurrencyPlan {
        source,
        install,
        switch: !busy,
        blocked: busy.then(|| {
            format!(
                "В этой установке уже есть оплаты в {}. Деньги из старого бота в {} перенесутся, но клиенты увидят их, только когда вы смените валюту в «Настройки → Общие».",
                currency_word(install),
                currency_word(source)
            )
        }),
    })
}

pub fn currency_word(currency: Currency) -> &'static str {
    match currency {
        Currency::Usd => "долларах",
        Currency::Rub => "рублях",
    }
}

fn method_word(method: Method) -> &'static str {
    match method {
        Method::Id => "по номеру пользователя",
        Method::ShortUuid => "по ссылке подписки",
        Method::Telegram => "по Telegram ID",
        Method::Email => "по почте",
        Method::Username => "по имени",
    }
}

pub struct Checked {
    pub preview: Preview,
    pub issues: Vec<IssueRow>,
}

pub fn problem_issues(legacy: &Legacy) -> Vec<IssueRow> {
    let mut issues: Vec<IssueRow> = legacy
        .problems
        .iter()
        .map(|p| IssueRow {
            stage: "check",
            code: p.code.to_string(),
            entity: p.entity.to_string(),
            old_id: p.old_id.clone(),
            who: p.who.clone(),
            details: p.details.clone(),
        })
        .collect();
    for tariff in &legacy.tariffs {
        if let Some(note) = &tariff.note {
            issues.push(IssueRow {
                stage: "check",
                code: "tariff_note".into(),
                entity: "tariff".into(),
                old_id: Some(tariff.old_id.clone()),
                who: Some(tariff.name.clone()),
                details: format!("Тариф «{}»: {note}", tariff.name),
            });
        }
    }
    issues
}

pub struct SubIssues<'a> {
    legacy: &'a Legacy,
    index: &'a RemnaIndex,
    clients: HashMap<&'a str, &'a LClient>,
}

impl<'a> SubIssues<'a> {
    pub fn new(legacy: &'a Legacy, index: &'a RemnaIndex) -> Self {
        SubIssues {
            legacy,
            index,
            clients: legacy
                .clients
                .iter()
                .map(|c| (c.old_id.as_str(), c))
                .collect(),
        }
    }

    fn who(&self, old_id: &str) -> Option<String> {
        self.clients.get(old_id).map(|c| c.who())
    }

    pub fn issue(&self, at: usize, matched: &Matched) -> Option<IssueRow> {
        let sub = &self.legacy.subscriptions[at];
        let (code, details) = match matched {
            Matched::Found(..) => return None,
            Matched::Missing => {
                let mut hints = Vec::new();
                if let Some(id) = sub.remna_id {
                    hints.push(format!("номер {id}"));
                }
                if let Some(short) = &sub.short_uuid {
                    hints.push(format!("ссылка …/{short}"));
                }
                if let Some(uuid) = &sub.remna_uuid {
                    hints.push(format!("uuid {uuid}"));
                }
                let expired = sub
                    .expire_at
                    .map(|at| format!(", срок до {}", at.format("%d.%m.%Y")))
                    .unwrap_or_default();
                let details = if hints.is_empty() {
                    format!("Пользователь Remnawave не найден{expired}")
                } else {
                    format!(
                        "Пользователь Remnawave не найден ({}){expired}",
                        hints.join(", ")
                    )
                };
                ("subscription_missing", details)
            }
            Matched::Ambiguous(1) => (
                "subscription_ambiguous",
                "Подходящий пользователь Remnawave подходит и другой подписке — выберите вручную в карточке клиента".to_string(),
            ),
            Matched::Ambiguous(n) => (
                "subscription_ambiguous",
                format!(
                    "Подходят сразу несколько пользователей Remnawave ({n}) — выберите вручную в карточке клиента"
                ),
            ),
            Matched::Shared(i, other) => {
                let name = &self.index.users[*i].username;
                let other = &self.legacy.subscriptions[*other];
                let details = if other.client_old_id == sub.client_old_id {
                    format!(
                        "В старой панели две подписки клиента вели к одному пользователю Remnawave {name} — перенесена одна"
                    )
                } else {
                    format!(
                        "В старой панели пользователь Remnawave {name} был общим с клиентом {}. Подписка перенесена тому клиенту, этому — нет",
                        self.who(&other.client_old_id)
                            .unwrap_or_else(|| "из той же выгрузки".into())
                    )
                };
                ("subscription_shared", details)
            }
        };
        Some(IssueRow {
            stage: "check",
            code: code.into(),
            entity: "subscription".into(),
            old_id: Some(sub.old_id.clone()),
            who: self.who(&sub.client_old_id),
            details,
        })
    }
}

pub async fn preview(
    services: &Services,
    legacy: &Legacy,
    remna: Result<Option<RemnaIndex>, String>,
) -> ServiceResult<Checked> {
    let mut issues = problem_issues(legacy);
    let mut skipped = legacy.skipped.clone();
    let source: Source = legacy.source;
    let clients_entity = source.entity("client");
    let mapped = legacy_map(services, &clients_entity).await?;
    let subs_map = legacy_map(services, &source.entity("subscription")).await?;
    let (by_tg, by_email) = identity_owners(services, legacy).await?;
    let mut existing = Existing::default();
    let mut target_client: HashMap<&str, i64> = HashMap::new();
    for client in &legacy.clients {
        if let Some((id, _)) = mapped.get(&client.old_id) {
            existing.again += 1;
            target_client.insert(&client.old_id, *id);
            continue;
        }
        let owner = client
            .telegram_id
            .and_then(|t| by_tg.get(&t))
            .or_else(|| client.email.as_ref().and_then(|e| by_email.get(e)));
        if let Some(owner) = owner {
            existing.matched += 1;
            target_client.insert(&client.old_id, *owner);
        }
    }
    {
        let mut conn = services.store().conn().await?;
        let total: i64 =
            sqlx::query_scalar("SELECT count(*) FROM clients WHERE deleted_at IS NULL")
                .fetch_one(sn_store::Db::conn(&mut conn))
                .await?;
        existing.clients = u64::try_from(total).unwrap_or(0);
    }
    let mut plan = RemnaPlan::default();
    match remna {
        Ok(Some(index)) => {
            plan.connected = true;
            plan.users = index.len() as u64;
            let matched = match_all(legacy, &index, &HashSet::new());
            let sub_issues = SubIssues::new(legacy, &index);
            let user_ids: Vec<i64> = matched
                .iter()
                .filter_map(|m| match m {
                    Matched::Found(i, _) => Some(index.users[*i].id.0),
                    _ => None,
                })
                .collect();
            let linked = links(services, &user_ids).await?;
            let mut methods: HashMap<Method, u64> = HashMap::new();
            let mut conflicts = 0u64;
            let mut expired_missing = 0u64;
            let now = services.now();
            for (at, m) in matched.iter().enumerate() {
                let sub = &legacy.subscriptions[at];
                match m {
                    Matched::Found(i, how) => {
                        let user = &index.users[*i];
                        let conflict = linked.get(&user.id.0).is_some_and(|(sub_id, client)| {
                            let ours = subs_map
                                .get(&sub.old_id)
                                .is_some_and(|(id, _)| id == sub_id);
                            let same_client = target_client
                                .get(sub.client_old_id.as_str())
                                .is_some_and(|c| c == client);
                            !ours && !same_client
                        });
                        if conflict {
                            conflicts += 1;
                            issues.push(IssueRow {
                                stage: "check",
                                code: "subscription_taken".into(),
                                entity: "subscription".into(),
                                old_id: Some(sub.old_id.clone()),
                                who: None,
                                details: format!(
                                    "Пользователь Remnawave {} уже привязан к другому клиенту этой установки",
                                    user.username
                                ),
                            });
                        } else {
                            plan.found += 1;
                            *methods.entry(*how).or_default() += 1;
                        }
                    }
                    Matched::Missing => {
                        plan.missing += 1;
                        if sub.expire_at.is_none_or(|at| at < now) {
                            expired_missing += 1;
                        }
                        issues.extend(sub_issues.issue(at, m));
                    }
                    Matched::Ambiguous(_) => {
                        plan.ambiguous += 1;
                        issues.extend(sub_issues.issue(at, m));
                    }
                    Matched::Shared(..) => {
                        plan.shared += 1;
                        issues.extend(sub_issues.issue(at, m));
                    }
                }
            }
            if plan.missing > 0 {
                let tail = if expired_missing == plan.missing {
                    " Все они уже истекли."
                } else if expired_missing > 0 {
                    " Часть из них уже истекла."
                } else {
                    ""
                };
                skipped.push(super::model::Skip {
                    code: "subscription_missing".into(),
                    count: plan.missing,
                    text: format!(
                        "подписок нет в Remnawave: пользователя удалили из панели или это другая панель. Клиенты перенесутся без этих подписок.{tail}"
                    ),
                });
            }
            if plan.ambiguous > 0 {
                skipped.push(super::model::Skip {
                    code: "subscription_ambiguous".into(),
                    count: plan.ambiguous,
                    text: "подписок нельзя однозначно сопоставить с пользователями Remnawave. Их не привязываем: выберите вручную в карточке клиента, список в отчёте.".into(),
                });
            }
            if plan.shared > 0 {
                skipped.push(super::model::Skip {
                    code: "subscription_shared".into(),
                    count: plan.shared,
                    text: "подписок в старой панели вели к тому же пользователю Remnawave, что и подписка другого клиента или вторая подписка того же клиента. Один пользователь Remnawave — одна подписка: переносим её один раз, список в отчёте.".into(),
                });
            }
            if conflicts > 0 {
                skipped.push(super::model::Skip {
                    code: "subscription_taken".into(),
                    count: conflicts,
                    text: "подписок уже привязаны в этой установке к другим клиентам. Их не трогаем — список в отчёте.".into(),
                });
            }
            let mut how: Vec<(Method, u64)> = methods.into_iter().collect();
            how.sort_by_key(|(m, _)| *m as u8);
            tracing::debug!(
                found = plan.found,
                by = %how
                    .iter()
                    .map(|(m, n)| format!("{} {n}", method_word(*m)))
                    .collect::<Vec<_>>()
                    .join(", "),
                "сопоставление подписок с Remnawave"
            );
        }
        Ok(None) => {}
        Err(err) => plan.error = Some(err),
    }
    let currency = currency_plan(services, legacy.currency).await?;
    let mut positive: Vec<i64> = legacy
        .clients
        .iter()
        .map(|c| c.balance_minor)
        .filter(|b| *b > 0)
        .collect();
    let (large_from, by_tariff) = large_balance_from(legacy, &mut positive).unzip();
    let mut large = (0u64, 0i64);
    if let Some(from) = large_from {
        for client in legacy.clients.iter().filter(|c| c.balance_minor > from) {
            large.0 += 1;
            large.1 = large.1.saturating_add(client.balance_minor);
            issues.push(IssueRow {
                stage: "check",
                code: "balance_large".into(),
                entity: "client".into(),
                old_id: Some(client.old_id.clone()),
                who: Some(client.who()),
                details: format!(
                    "Баланс {} {} — подозрительно большой, порог {}. Если это ошибка, исправьте баланс в старой панели и перенесите заново, иначе он перенесётся как есть",
                    super::stealthnet::format_minor(client.balance_minor),
                    legacy.currency.code(),
                    super::stealthnet::format_minor(from)
                ),
            });
        }
    }
    let preview = Preview {
        detected: legacy.source,
        dumped_at: legacy.dumped_at,
        counts: Counts {
            clients: legacy.clients.len() as u64,
            subscriptions: legacy.subscriptions.len() as u64,
            tariffs: legacy.tariffs.len() as u64,
            promo_codes: legacy.promos.len() as u64,
            referrals: legacy.referrals() as u64,
            payments: legacy.payments.len() as u64,
            tickets: legacy.tickets.len() as u64,
        },
        balances: Balances {
            clients: positive.len() as u64,
            total_minor: positive.iter().sum(),
            currency: legacy.currency,
            large_clients: large.0,
            large_total_minor: large.1,
            large_from_minor: large_from.filter(|_| large.0 > 0),
            large_by_tariff: by_tariff.unwrap_or(false),
        },
        currency,
        remna: plan,
        existing,
        tariff_mode: legacy.tariff_mode,
        skipped,
    };
    Ok(Checked { preview, issues })
}

fn large_balance_from(legacy: &Legacy, positive: &mut [i64]) -> Option<(i64, bool)> {
    let priciest = legacy
        .tariffs
        .iter()
        .filter(|t| t.currency == legacy.currency)
        .flat_map(|t| t.periods.iter().filter_map(|p| p.price_minor))
        .filter(|p| *p > 0)
        .max();
    if let Some(price) = priciest {
        return Some((price.saturating_mul(10), true));
    }
    if positive.len() < 100 {
        return None;
    }
    positive.sort_unstable();
    let usual = positive[positive.len() * 99 / 100];
    Some((usual.saturating_mul(10), false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn порог_крупного_баланса_без_цен_берётся_из_самих_балансов() {
        let legacy = Legacy::new(Source::Bedolaga, Currency::Rub);
        let mut few: Vec<i64> = vec![100; 99];
        assert_eq!(large_balance_from(&legacy, &mut few), None);
        let mut many: Vec<i64> = (1..=200).rev().map(|v| v * 100).collect();
        assert_eq!(
            large_balance_from(&legacy, &mut many),
            Some((199_000, false))
        );
    }
}
