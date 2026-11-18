use super::facts::crypto_cond;
use super::labels;
use super::query::{Filters, SourceFilter, Sql, bound_after, midnight, rows};
use super::spans::GRACE_DAYS;
use super::{TabQuery, offset};
use crate::catalog::web::WebError;
use crate::context::ModuleContext;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sn_store::Db;

const PAGE: i64 = 50;

#[derive(Debug, Default, Clone, Deserialize)]
pub struct ListQuery {
    pub set: String,
    #[serde(default)]
    pub slice: Option<String>,
    #[serde(default)]
    pub from: Option<NaiveDate>,
    #[serde(default)]
    pub to: Option<NaiveDate>,
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub tariff: Option<i64>,
    #[serde(default)]
    pub category: Option<i64>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
}

impl ListQuery {
    fn filters(&self) -> Filters {
        TabQuery {
            tariff: self.tariff,
            category: self.category,
            channel: self.channel.clone(),
            source: self.source.clone(),
            provider: self.provider.clone(),
            method: self.method.clone(),
            ..TabQuery::default()
        }
        .filters()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListItem {
    pub client_id: i64,
    pub name: String,
    pub what: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<i64>,
    pub stars: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListPage {
    pub kind: &'static str,
    pub total: i64,
    pub page: i64,
    pub more: bool,
    pub items: Vec<ListItem>,
}

const NAME: &str = "COALESCE(NULLIF(btrim(concat_ws(' ', tg.meta ->> 'first_name', tg.meta ->> 'last_name')), ''), \
     '@' || (tg.meta ->> 'username'), em.value, 'клиент №' || x.client_id)";
const NAME_JOIN: &str = "LEFT JOIN LATERAL (SELECT i.meta FROM client_identities i WHERE i.client_id = x.client_id \
       AND i.kind = 'telegram' ORDER BY i.id LIMIT 1) tg ON true \
     LEFT JOIN LATERAL (SELECT i.value FROM client_identities i WHERE i.client_id = x.client_id \
       AND i.kind = 'email' ORDER BY i.id LIMIT 1) em ON true";

fn bad(text: &str) -> WebError {
    WebError::rejected("analytics.list", text.to_string())
}

fn kind_cond(key: &str) -> Option<String> {
    Some(match key {
        "new" | "renewal" | "change" | "topup" | "trial" => format!("f.kind = '{key}'"),
        "devices" => "f.item = 'options:devices'".into(),
        "traffic" => "f.item = 'options:traffic'".into(),
        "gift" => "f.item = 'gifts:gift'".into(),
        "other" => "f.kind NOT IN ('new', 'renewal', 'change', 'topup', 'trial') \
                    AND f.item NOT IN ('options:devices', 'options:traffic', 'gifts:gift')"
            .into(),
        _ => return None,
    })
}

fn payment_slice(sql: &mut Sql, slice: &str) -> Result<String, WebError> {
    let (dim, value) = slice
        .split_once(':')
        .ok_or_else(|| bad("Разрез указан неверно"))?;
    Ok(match dim {
        "kind" => kind_cond(value).ok_or_else(|| bad("Такого вида оплаты нет"))?,
        "tariff" if value == "none" => "f.tariff_id IS NULL".into(),
        "tariff" => {
            let id: i64 = value.parse().map_err(|_| bad("Тариф указан неверно"))?;
            format!("f.tariff_id = {}", sql.int(id))
        }
        "period" if value == "none" => "f.period_days IS NULL".into(),
        "period" => {
            let days: i64 = value.parse().map_err(|_| bad("Срок указан неверно"))?;
            format!("f.period_days = {}", sql.int(days))
        }
        "provider" => format!("f.provider = {}", sql.text(value)),
        "method" => {
            let (p, m) = value.split_once('/').unwrap_or((value, ""));
            format!(
                "f.provider = {} AND f.method = {}",
                sql.text(p),
                sql.text(m)
            )
        }
        "channel" => {
            let v = if value == "unknown" { "" } else { value };
            format!("f.channel = {}", sql.text(v))
        }
        _ => return Err(bad("Такого разреза нет")),
    })
}

type PayRow = (
    i64,
    String,
    DateTime<Utc>,
    String,
    String,
    Option<i64>,
    Option<i32>,
    String,
    String,
    i64,
    bool,
    i64,
    i64,
);

fn describe(ctx: &ModuleContext, names: &labels::Names, r: &PayRow) -> String {
    let (_, _, _, kind, item, tariff, period, provider, method, _, _, refunded, _) = r;
    let titles = ctx.core.payment_titles();
    let mut parts = vec![labels::kind(kind, item).1.to_string()];
    if tariff.is_some() {
        parts.push(names.tariff(*tariff));
    }
    if period.is_some() {
        parts.push(labels::period(period.map(i64::from)));
    }
    let way = if method.is_empty() {
        labels::provider(&titles, provider)
    } else {
        format!(
            "{} · {}",
            labels::provider(&titles, provider),
            labels::method(&titles, provider, method)
        )
    };
    parts.push(way);
    if *refunded > 0 {
        parts.push(format!(
            "возвращено {}",
            labels::money_short(*refunded as f64, ctx.currency)
        ));
    }
    parts.join(" · ")
}

async fn payments(
    ctx: &ModuleContext,
    q: &ListQuery,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<ListPage, WebError> {
    let page = q.page.unwrap_or(0).max(0);
    let mut sql = Sql::default();
    let s = sql.time(start);
    let e = sql.time(end);
    let f = q.filters().facts(&mut sql, "f");
    let base = match q.set.as_str() {
        "revenue" => "NOT f.stars AND f.provider <> 'balance'".to_string(),
        "promo" => "NOT f.stars AND f.promo_minor > 0".to_string(),
        "stars" => "f.stars".to_string(),
        "crypto" => format!(
            "NOT f.stars AND f.provider <> 'balance' AND {}",
            crypto_cond("f")
        ),
        "balance" => "f.provider = 'balance'".to_string(),
        "refunds" => "f.refunded_minor > 0".to_string(),
        _ => return Err(bad("Такого списка нет")),
    };
    let slice = match q.slice.as_deref() {
        Some(sl) => payment_slice(&mut sql, sl)?,
        None => "true".into(),
    };
    let limit = sql.int(PAGE);
    let skip = sql.int(page * PAGE);
    let text = format!(
        "WITH x AS (SELECT f.* FROM analytics_payments f \
           WHERE f.paid_at >= {s} AND f.paid_at < {e} AND {base} AND {slice} AND {f}) \
         SELECT x.client_id, {NAME}, x.paid_at, x.kind, x.item, x.tariff_id, x.period_days, x.provider, x.method, \
           x.amount_minor, x.stars, x.refunded_minor, count(*) OVER ()::bigint \
         FROM x {NAME_JOIN} ORDER BY x.paid_at DESC LIMIT {limit} OFFSET {skip}"
    );
    let mut conn = ctx.store.conn().await?;
    let names = labels::names(&mut conn).await?;
    let found: Vec<PayRow> = rows(&mut conn, &sql, &text).await?;
    let total = found.first().map_or(0, |r| r.12);
    Ok(ListPage {
        kind: "payments",
        total,
        page,
        more: (page + 1) * PAGE < total,
        items: found
            .iter()
            .map(|r| ListItem {
                client_id: r.0,
                name: r.1.clone(),
                what: describe(ctx, &names, r),
                amount: Some(r.9),
                stars: r.10,
                at: Some(r.2),
            })
            .collect(),
    })
}

type InvoiceRow = (
    i64,
    String,
    DateTime<Utc>,
    String,
    String,
    String,
    i64,
    String,
    Option<String>,
    i64,
);

async fn invoices(
    ctx: &ModuleContext,
    q: &ListQuery,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<ListPage, WebError> {
    let page = q.page.unwrap_or(0).max(0);
    let mut sql = Sql::default();
    let s = sql.time(start);
    let e = sql.time(end);
    let fc = q.filters().client(&mut sql, "a.client_id");
    let state = match q.set.as_str() {
        "invoices" => "true",
        "abandoned" => "(a.status = 'canceled' OR (a.status = 'pending' AND a.expires_at < now()))",
        _ => return Err(bad("Такого списка нет")),
    };
    let slice = match q.slice.as_deref().and_then(|s| s.split_once(':')) {
        Some(("provider", p)) => format!("a.provider = {}", sql.text(p)),
        Some(("method", v)) => {
            let (p, m) = v.split_once('/').unwrap_or((v, ""));
            format!(
                "a.provider = {} AND a.method = {}",
                sql.text(p),
                sql.text(m)
            )
        }
        _ => "true".into(),
    };
    let limit = sql.int(PAGE);
    let skip = sql.int(page * PAGE);
    let found_in = format!(
        "analytics_attempts a WHERE a.created_at >= {s} AND a.created_at < {e} AND {state} AND {slice} AND {fc}"
    );
    let text = format!(
        "WITH x AS (SELECT a.payment_id, a.client_id, a.created_at, a.status, a.provider, a.method FROM {found_in} \
           ORDER BY a.created_at DESC, a.payment_id DESC LIMIT {limit} OFFSET {skip}) \
         SELECT x.client_id, {NAME}, x.created_at, x.status, x.provider, x.method, p.amount_minor, p.currency, \
           p.failure_reason, (SELECT count(*) FROM {found_in})::bigint \
         FROM x JOIN payments p ON p.id = x.payment_id {NAME_JOIN} \
         ORDER BY x.created_at DESC, x.payment_id DESC"
    );
    let mut conn = ctx.store.conn().await?;
    let found: Vec<InvoiceRow> = rows(&mut conn, &sql, &text).await?;
    let titles = ctx.core.payment_titles();
    let total = found.first().map_or(0, |r| r.9);
    Ok(ListPage {
        kind: "payments",
        total,
        page,
        more: (page + 1) * PAGE < total,
        items: found
            .into_iter()
            .map(
                |(client_id, name, at, status, provider, method, amount, currency, reason, _)| {
                    let state = match status.as_str() {
                        "paid" | "partially_refunded" | "refunded" => "оплачен".to_string(),
                        "failed" => format!(
                            "ошибка{}",
                            reason.map(|r| format!(": {r}")).unwrap_or_default()
                        ),
                        "canceled" => "брошен".into(),
                        _ => "ждёт оплаты".into(),
                    };
                    let way = if method.is_empty() {
                        labels::provider(&titles, &provider)
                    } else {
                        format!(
                            "{} · {}",
                            labels::provider(&titles, &provider),
                            labels::method(&titles, &provider, &method)
                        )
                    };
                    ListItem {
                        client_id,
                        name,
                        what: format!("{way} · {state}"),
                        amount: Some(amount),
                        stars: currency == "XTR",
                        at: Some(at),
                    }
                },
            )
            .collect(),
    })
}

fn client_set(
    sql: &mut Sql,
    q: &ListQuery,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<String, WebError> {
    let filters = q.filters();
    let fc = |sql: &mut Sql, col: &str| filters.client(sql, col);
    let s = sql.time(start);
    let e = sql.time(end);
    let n = sql.time(now);
    let slice = q.slice.clone().unwrap_or_default();
    let (dim, value) = slice.split_once(':').unwrap_or(("", ""));
    Ok(match q.set.as_str() {
        "registered" | "source" => {
            let mut cond = filters.source_cond(sql, "ac");
            match (q.set.as_str(), dim) {
                ("registered", "channel") => {
                    cond.push_str(&format!(" AND ac.reg_channel = {}", sql.text(value)))
                }
                ("registered", "login") => {
                    cond.push_str(&format!(" AND ac.login_kind = {}", sql.text(value)))
                }
                ("registered", "source") => {
                    cond.push_str(&format!(" AND ac.source_type = {}", sql.text(value)))
                }
                ("source", _) => {
                    let only = Filters {
                        source: SourceFilter::parse(&slice),
                        ..Filters::default()
                    };
                    cond.push_str(&format!(" AND {}", only.source_cond(sql, "ac")));
                }
                _ => {}
            }
            format!(
                "SELECT ac.client_id, ac.created_at AS at, \
                   'Регистрация · ' || CASE ac.reg_channel WHEN 'bot' THEN 'бот' WHEN 'miniapp' THEN 'мини-приложение' \
                     WHEN 'web' THEN 'сайт' ELSE ac.reg_channel END \
                   || CASE WHEN ac.source_type = 'utm' THEN ' · ' || ac.source_key || \
                     CASE WHEN ac.utm_campaign <> '' THEN ' / ' || ac.utm_campaign ELSE '' END ELSE '' END AS what, \
                   NULL::bigint AS amount \
                 FROM analytics_clients ac WHERE ac.created_at >= {s} AND ac.created_at < {e} AND NOT ac.deleted AND {cond}"
            )
        }
        "trials" => {
            let f = fc(sql, "o.client_id");
            format!(
                "SELECT o.client_id, o.paid_at AS at, \
                   CASE WHEN b.first IS NULL THEN 'Пробный период, без покупки' \
                     ELSE 'Пробный период, купил через ' || greatest(0, extract(day FROM b.first - o.paid_at)::int) || ' дн.' END AS what, \
                   NULL::bigint AS amount \
                 FROM orders o LEFT JOIN LATERAL (SELECT min(sp.paid_at) AS first FROM analytics_spans sp \
                   WHERE sp.client_id = o.client_id AND sp.paid_at >= o.paid_at) b ON true \
                 WHERE o.kind = 'trial' AND o.status IN ('paid', 'fulfilled', 'review') \
                   AND o.paid_at >= {s} AND o.paid_at < {e} AND {f}"
            )
        }
        "active" | "autorenew_on" | "autorenew_off" | "expiring7" | "expiring30" => {
            let f = fc(sql, "sb.client_id");
            let t = filters.tariff_cond(sql, "sb.tariff_id");
            let mut cond = format!(
                "sb.ended_at IS NULL AND COALESCE(sb.status, 'active') IN ('active', 'limited') \
                 AND (sb.expire_at IS NULL OR sb.expire_at > {n}) AND {f} AND {t}"
            );
            match q.set.as_str() {
                "autorenew_on" => cond.push_str(" AND sb.kind = 'paid' AND sb.autorenew_enabled"),
                "autorenew_off" => {
                    cond.push_str(" AND sb.kind = 'paid' AND NOT sb.autorenew_enabled")
                }
                "expiring7" | "expiring30" => {
                    let days = if q.set == "expiring7" { 7 } else { 30 };
                    let till = sql.time(now + Duration::days(days));
                    cond.push_str(&format!(" AND sb.kind = 'paid' AND sb.expire_at <= {till}"));
                    match value {
                        "on" => cond.push_str(" AND sb.autorenew_enabled"),
                        "off" => cond.push_str(" AND NOT sb.autorenew_enabled"),
                        _ => {}
                    }
                }
                _ => match dim {
                    "kind" => cond.push_str(&format!(" AND sb.kind = {}", sql.text(value))),
                    "tariff" if value == "none" => cond.push_str(" AND sb.tariff_id IS NULL"),
                    "tariff" => {
                        let id: i64 = value.parse().map_err(|_| bad("Тариф указан неверно"))?;
                        cond.push_str(&format!(" AND sb.tariff_id = {}", sql.int(id)));
                    }
                    _ => {}
                },
            }
            format!(
                "SELECT sb.client_id, sb.expire_at AS at, \
                   'Подписка до ' || COALESCE(to_char(sb.expire_at, 'DD.MM.YYYY'), 'без срока') \
                   || CASE WHEN sb.autorenew_enabled THEN ', автопродление' ELSE '' END AS what, NULL::bigint AS amount \
                 FROM subscriptions sb WHERE {cond}"
            )
        }
        "balance" | "referral_balance" => {
            let f = fc(sql, "b.client_id");
            let wallet = if q.set == "balance" {
                "main"
            } else {
                "referral"
            };
            format!(
                "SELECT b.client_id, b.updated_at AS at, 'Баланс' AS what, b.amount_minor AS amount \
                 FROM client_balances b WHERE b.wallet = '{wallet}' AND b.amount_minor > 0 AND {f}"
            )
        }
        "blocked" => {
            let f = fc(sql, "cl.id");
            format!(
                "SELECT cl.id AS client_id, cl.blocked_at AS at, COALESCE('Заблокирован: ' || cl.block_reason, 'Заблокирован') AS what, \
                   NULL::bigint AS amount FROM clients cl WHERE cl.status = 'blocked' AND cl.deleted_at IS NULL AND {f}"
            )
        }
        "renewed_on_time" | "renewed_late" => {
            let f = filters.spans(sql, "sp");
            let cond = if q.set == "renewed_on_time" {
                "sp.gap_seconds <= 0"
            } else {
                "sp.gap_seconds > 0"
            };
            format!(
                "SELECT sp.client_id, sp.paid_at AS at, \
                   CASE WHEN sp.gap_seconds > 0 THEN 'Продление с опозданием на ' || ceil(sp.gap_seconds / 86400.0)::int || ' дн.' \
                     ELSE 'Продление вовремя' END AS what, NULL::bigint AS amount \
                 FROM analytics_spans sp WHERE sp.kind = 'renewal' AND sp.continues AND {cond} \
                   AND sp.paid_at >= {s} AND sp.paid_at < {e} AND {f}"
            )
        }
        "returned" => {
            let f = fc(sql, "i.client_id");
            format!(
                "SELECT i.client_id, i.starts_at AS at, 'Вернулся после перерыва' AS what, NULL::bigint AS amount \
                 FROM analytics_islands i WHERE i.seq > 1 AND i.starts_at >= {s} AND i.starts_at < {e} AND {f}"
            )
        }
        "churned" => {
            let f = fc(sql, "i.client_id");
            let g = sql.time(now - Duration::days(GRACE_DAYS));
            format!(
                "SELECT i.client_id, i.ends_at AS at, 'Подписка закончилась и не продлена' AS what, NULL::bigint AS amount \
                 FROM analytics_islands i WHERE i.ends_at >= {s} AND i.ends_at < {e} AND i.ends_at < {g} AND {f}"
            )
        }
        "mrr_new" | "mrr_up" | "mrr_down" | "mrr_churn" | "mrr_back" => {
            let fa = filters.spans(sql, "s");
            let t1 = sql.time(end.min(now));
            let (class, label) = match q.set.as_str() {
                "mrr_new" => ("j.m0 = 0 AND j.m1 > 0 AND NOT j.earlier", "Новый платящий"),
                "mrr_up" => ("j.m0 > 0 AND j.m1 > j.m0", "Стал платить больше"),
                "mrr_down" => ("j.m1 > 0 AND j.m1 < j.m0", "Стал платить меньше"),
                "mrr_churn" => ("j.m0 > 0 AND j.m1 = 0", "Ушёл"),
                _ => ("j.m0 = 0 AND j.m1 > 0 AND j.earlier", "Вернулся"),
            };
            format!(
                "WITH a AS (SELECT s.client_id, sum(s.monthly_minor) AS m FROM analytics_spans s \
                     WHERE s.starts_at < {s} AND s.ends_at >= {s} AND {fa} GROUP BY 1), \
                   b AS (SELECT s.client_id, sum(s.monthly_minor) AS m FROM analytics_spans s \
                     WHERE s.starts_at < {t1} AND s.ends_at >= {t1} AND {fa} GROUP BY 1), \
                   j AS (SELECT COALESCE(a.client_id, b.client_id) AS client_id, COALESCE(a.m, 0) AS m0, COALESCE(b.m, 0) AS m1, \
                     EXISTS (SELECT 1 FROM analytics_spans x WHERE x.client_id = COALESCE(a.client_id, b.client_id) \
                       AND x.starts_at < {s}) AS earlier FROM a FULL JOIN b ON a.client_id = b.client_id) \
                 SELECT j.client_id, NULL::timestamptz AS at, '{label}' AS what, \
                   (j.m1 - j.m0)::bigint AS amount FROM j WHERE {class}"
            )
        }
        _ => return Err(bad("Такого списка нет")),
    })
}

type ClientRow = (i64, String, String, Option<i64>, Option<DateTime<Utc>>, i64);

async fn clients(
    ctx: &ModuleContext,
    q: &ListQuery,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<ListPage, WebError> {
    let page = q.page.unwrap_or(0).max(0);
    let mut sql = Sql::default();
    let set = client_set(&mut sql, q, start, end, now)?;
    let limit = sql.int(PAGE);
    let skip = sql.int(page * PAGE);
    let text = format!(
        "WITH x AS ({set}) \
         SELECT x.client_id, {NAME}, x.what, x.amount, x.at, count(*) OVER ()::bigint \
         FROM x {NAME_JOIN} ORDER BY x.at DESC NULLS LAST, x.client_id DESC LIMIT {limit} OFFSET {skip}"
    );
    let mut conn = ctx.store.conn().await?;
    let found: Vec<ClientRow> = rows(&mut conn, &sql, &text).await?;
    let total = found.first().map_or(0, |r| r.5);
    let money = q.set.starts_with("mrr_") || q.set.ends_with("balance");
    Ok(ListPage {
        kind: "clients",
        total,
        page,
        more: (page + 1) * PAGE < total,
        items: found
            .into_iter()
            .map(|(client_id, name, what, amount, at, _)| ListItem {
                client_id,
                name,
                what,
                amount: if money { amount } else { None },
                stars: false,
                at,
            })
            .collect(),
    })
}

pub async fn page(
    ctx: &ModuleContext,
    q: &ListQuery,
    now: DateTime<Utc>,
) -> Result<ListPage, WebError> {
    let offset = {
        let mut conn = ctx.store.conn().await?;
        offset(&mut conn).await?
    };
    let today = super::query::local_day(now, offset);
    let to = q.to.unwrap_or(today);
    let from = q.from.unwrap_or(to);
    if from > to {
        return Err(bad("Начало периода позже конца"));
    }
    let start = midnight(from, offset);
    let end = bound_after(to, offset);
    match q.set.as_str() {
        "revenue" | "promo" | "stars" | "crypto" | "balance" | "refunds" => {
            payments(ctx, q, start, end).await
        }
        "invoices" | "abandoned" => invoices(ctx, q, start, end).await,
        _ => clients(ctx, q, start, end, now).await,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Options {
    pub tariffs: Vec<Choice>,
    pub categories: Vec<Choice>,
    pub channels: Vec<Choice>,
    pub sources: Vec<Choice>,
    pub providers: Vec<Choice>,
    pub methods: Vec<Choice>,
}

pub async fn options(ctx: &ModuleContext) -> Result<Options, WebError> {
    let mut conn = ctx.store.conn().await?;
    let tariffs: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, name FROM tariffs WHERE deleted_at IS NULL ORDER BY sort, id")
            .fetch_all(conn.conn())
            .await
            .map_err(sn_store::StoreError::from)?;
    let categories: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, name FROM tariff_categories WHERE deleted_at IS NULL ORDER BY sort, id",
    )
    .fetch_all(conn.conn())
    .await
    .map_err(sn_store::StoreError::from)?;
    let utm: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT source FROM client_utm WHERE source IS NOT NULL ORDER BY 1 LIMIT 300",
    )
    .fetch_all(conn.conn())
    .await
    .map_err(sn_store::StoreError::from)?;
    let links: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, name FROM promo_links ORDER BY id DESC LIMIT 300")
            .fetch_all(conn.conn())
            .await
            .map_err(sn_store::StoreError::from)?;
    let used: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT provider, method FROM analytics_payments WHERE provider <> 'balance' ORDER BY 1, 2",
    )
    .fetch_all(conn.conn())
    .await
    .map_err(sn_store::StoreError::from)?;
    let titles = ctx.core.payment_titles();
    let mut providers: Vec<Choice> = Vec::new();
    let mut methods: Vec<Choice> = Vec::new();
    for (p, m) in &used {
        if !providers.iter().any(|c| &c.value == p) {
            providers.push(Choice {
                value: p.clone(),
                label: labels::provider(&titles, p),
            });
        }
        if !m.is_empty() {
            methods.push(Choice {
                value: format!("{p}/{m}"),
                label: format!(
                    "{} · {}",
                    labels::provider(&titles, p),
                    labels::method(&titles, p, m)
                ),
            });
        }
    }
    let mut sources = vec![
        Choice {
            value: "none".into(),
            label: "Без источника".into(),
        },
        Choice {
            value: "ref".into(),
            label: "Рефералы".into(),
        },
        Choice {
            value: "gift".into(),
            label: "Подарки".into(),
        },
    ];
    sources.extend(utm.into_iter().map(|(s,)| Choice {
        value: format!("utm:{s}"),
        label: format!("Метка {s}"),
    }));
    sources.extend(links.into_iter().map(|(id, name)| Choice {
        value: format!("promo:{id}"),
        label: format!("Промо-ссылка «{name}»"),
    }));
    Ok(Options {
        tariffs: tariffs
            .into_iter()
            .map(|(id, name)| Choice {
                value: id.to_string(),
                label: name,
            })
            .collect(),
        categories: categories
            .into_iter()
            .map(|(id, name)| Choice {
                value: id.to_string(),
                label: name,
            })
            .collect(),
        channels: ["bot", "miniapp", "cabinet", "autorenew", "admin", "unknown"]
            .iter()
            .map(|c| Choice {
                value: (*c).to_string(),
                label: labels::channel(if *c == "unknown" { "" } else { c }).to_string(),
            })
            .collect(),
        sources,
        providers,
        methods,
    })
}
