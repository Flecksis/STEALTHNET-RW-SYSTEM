use super::{Alert, AlertsOps, amount_text, client_label, cut, escape};
use crate::clients::{OrderBrief, method_title, order_brief};
use crate::error::ServiceResult;
use sn_domain::alerts::{AlertKind, AlertsConfig};
use sn_domain::client::{Locale, RegistrationSource};
use sn_domain::events::DomainEvent;
use sn_domain::format;
use sn_domain::ids::{ClientId, OrderId, PaymentId, TariffId};
use sn_domain::order::{Order, OrderKind};
use sn_domain::payment::{Payment, PaymentStatus};
use sn_modules::catalog::start_param::{Utm, referral_code};
use sn_modules::catalog::{gifts, promolinks};
use sn_store::{Db, RepeatAction};
use std::collections::HashMap;

pub const BATCH_LINES: usize = 10;
pub(super) const CLIENTS_CURSOR: &str = "alerts.cursor.clients";
pub(super) const SALES_CURSOR: &str = "alerts.cursor.sales";
const EVENTS_SETTLE_SECONDS: f64 = 10.0;
const NAME_MAX: usize = 48;
const LABEL_MAX: usize = 60;
const SELF_SIGNUP: [&str; 3] = ["bot", "miniapp", "web"];
const RANGE: &str = "FROM domain_events WHERE id > $1 AND id <= $2 AND kind = $3 \
     AND ($4::text[] IS NULL OR payload->>'source' = ANY($4))";

struct Batch {
    events: Vec<DomainEvent>,
    total: usize,
    newest: i64,
}

fn channel_text(source: RegistrationSource) -> &'static str {
    match source {
        RegistrationSource::Bot => "бот",
        RegistrationSource::Miniapp => "мини-приложение",
        RegistrationSource::Web => "кабинет",
        RegistrationSource::Admin => "добавлен в админке",
        RegistrationSource::Import | RegistrationSource::Migration => "перенос",
    }
}

fn client_ref(base: Option<&str>, id: ClientId, text: &str) -> String {
    match base {
        Some(base) => format!("<a href=\"{base}/admin/clients/{}\">{text}</a>", id.0),
        None => text.to_string(),
    }
}

fn utm_text(utm: &Utm) -> String {
    let parts: Vec<String> = [&utm.source, &utm.medium, &utm.campaign]
        .into_iter()
        .flatten()
        .map(|part| escape(&cut(part, 24)))
        .collect();
    format!("UTM: {}", parts.join(" / "))
}

fn bought_text(brief: &OrderBrief) -> String {
    let tariff = brief
        .tariff_name
        .as_deref()
        .map(|name| format!("«{}»", escape(&cut(name, LABEL_MAX))));
    let term = brief.days.map(|days| format::term_in(days, Locale::Ru));
    let with_term = |base: String| match &term {
        Some(term) => format!("{base} на {term}"),
        None => base,
    };
    match brief.kind {
        OrderKind::TopUp => "пополнение баланса".to_string(),
        OrderKind::Change => match tariff {
            Some(tariff) => format!("переход на {tariff}"),
            None => "смена тарифа".to_string(),
        },
        OrderKind::Renewal => with_term(format!(
            "продление {}",
            tariff.unwrap_or_else(|| "подписки".to_string())
        )),
        OrderKind::Module | OrderKind::ModuleGrant => brief
            .label
            .as_deref()
            .map(|label| escape(&cut(label, LABEL_MAX)))
            .unwrap_or_else(|| "дополнение".to_string()),
        OrderKind::New | OrderKind::Trial | OrderKind::AdminGrant => {
            let text = with_term(tariff.unwrap_or_else(|| "подписка".to_string()));
            if brief.extra_devices > 0 {
                format!(
                    "{text} + {} доп.",
                    format::devices_in(brief.extra_devices, Locale::Ru)
                )
            } else {
                text
            }
        }
    }
}

fn marks(
    payment: &Payment,
    refunded_minor: i64,
    repeat: Option<RepeatAction>,
) -> Vec<&'static str> {
    let mut marks = Vec::new();
    if payment.method.as_deref() == Some(crate::autorenew::AUTORENEW_METHOD) {
        marks.push("автопродление");
    }
    match repeat {
        Some(RepeatAction::Balance) => marks.push("повторная оплата, зачислена на баланс"),
        Some(RepeatAction::Review) => marks.push("повторная оплата, ждёт решения"),
        None => {}
    }
    if payment.status == PaymentStatus::Refunded {
        marks.push("уже возвращена");
    } else if refunded_minor > 0 || payment.status == PaymentStatus::PartiallyRefunded {
        marks.push("частично возвращена");
    }
    marks
}

impl AlertsOps<'_> {
    fn admin_base(&self) -> Option<String> {
        let settings = self.services.settings();
        settings
            .public_url
            .as_deref()
            .map(|url| url.trim().trim_end_matches('/'))
            .filter(|url| url.starts_with("https://"))
            .map(|url| url.replace('"', "%22"))
    }

    async fn batch(
        &self,
        key: &str,
        kind: &str,
        sources: Option<&[&str]>,
    ) -> ServiceResult<Option<Batch>> {
        let cursor = self.state(key).await?.and_then(|v| v.as_i64());
        let mut conn = self.services.store().conn().await?;
        let newest: i64 = sqlx::query_scalar(
            "SELECT coalesce(max(id), 0) FROM domain_events \
             WHERE occurred_at <= now() - make_interval(secs => $1)",
        )
        .bind(EVENTS_SETTLE_SECONDS)
        .fetch_one(conn.conn())
        .await?;
        let Some(cursor) = cursor else {
            drop(conn);
            self.put_state(key, &serde_json::json!(newest)).await?;
            return Ok(None);
        };
        if newest <= cursor {
            return Ok(None);
        }
        let total: i64 = sqlx::query_scalar(&format!("SELECT count(*) {RANGE}"))
            .bind(cursor)
            .bind(newest)
            .bind(kind)
            .bind(sources)
            .fetch_one(conn.conn())
            .await?;
        let rows: Vec<(i64, serde_json::Value)> =
            sqlx::query_as(&format!("SELECT id, payload {RANGE} ORDER BY id LIMIT $5"))
                .bind(cursor)
                .bind(newest)
                .bind(kind)
                .bind(sources)
                .bind(BATCH_LINES as i64)
                .fetch_all(conn.conn())
                .await?;
        let events = rows
            .into_iter()
            .filter_map(|(id, payload)| match serde_json::from_value(payload) {
                Ok(event) => Some(event),
                Err(err) => {
                    tracing::warn!(event = id, error = %err, "событие не разобрано, в уведомлении пропущено");
                    None
                }
            })
            .collect();
        Ok(Some(Batch {
            events,
            total: usize::try_from(total).unwrap_or_default(),
            newest,
        }))
    }

    async fn origin(&self, db: &mut impl Db, client_id: ClientId, param: &str) -> Vec<String> {
        let mut parts = Vec::new();
        if let Some(code) = referral_code(param) {
            let referrer: Option<i64> =
                sqlx::query_scalar("SELECT referrer_id FROM referral_links WHERE client_id = $1")
                    .bind(client_id.0)
                    .fetch_optional(db.conn())
                    .await
                    .ok()
                    .flatten();
            parts.push(match referrer {
                Some(referrer) => format!("пригласил клиент №{referrer}"),
                None => format!("реферальная ссылка {}", escape(&code)),
            });
        }
        if let Some(code) = promolinks::start_code(param) {
            parts.push(format!("промо-ссылка {}", escape(&code)));
        }
        if gifts::start_code(param).is_some() {
            parts.push("подарок".to_string());
        }
        if let Some(utm) = Utm::parse(param) {
            parts.push(utm_text(&utm));
        }
        parts
    }

    pub(super) async fn new_clients(&self, config: &AlertsConfig) -> ServiceResult<Vec<Alert>> {
        let Some(batch) = self
            .batch(CLIENTS_CURSOR, "client_registered", Some(&SELF_SIGNUP[..]))
            .await?
        else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        if config.route(AlertKind::Clients).on && batch.total > 0 {
            let base = self.admin_base();
            let mut conn = self.services.store().conn().await?;
            let mut lines = vec![format!("<b>Новые клиенты: {}</b>", batch.total)];
            let mut shown = Vec::new();
            for event in &batch.events {
                let DomainEvent::ClientRegistered {
                    client_id,
                    source,
                    start_param,
                } = event
                else {
                    continue;
                };
                let name = escape(&cut(&client_label(&mut conn, client_id.0).await, NAME_MAX));
                let mut parts = vec![
                    name,
                    client_ref(base.as_deref(), *client_id, &format!("№{}", client_id.0)),
                    channel_text(*source).to_string(),
                ];
                if let Some(param) = start_param.as_deref() {
                    parts.extend(self.origin(&mut conn, *client_id, param).await);
                }
                lines.push(parts.join(" · "));
                shown.push(*client_id);
            }
            let rest = batch.total.saturating_sub(shown.len());
            if rest > 0 {
                lines.push(format!("и ещё {rest}"));
            }
            out.push(Alert {
                kind: AlertKind::Clients,
                html: lines.join("\n"),
                path: match shown.as_slice() {
                    [only] if batch.total == 1 => format!("/admin/clients/{}", only.0),
                    _ => "/admin/clients".to_string(),
                },
            });
        }
        self.put_state(CLIENTS_CURSOR, &serde_json::json!(batch.newest))
            .await?;
        Ok(out)
    }

    pub(super) async fn new_sales(&self, config: &AlertsConfig) -> ServiceResult<Vec<Alert>> {
        let Some(batch) = self.batch(SALES_CURSOR, "payment_paid", None).await? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        if config.route(AlertKind::Sales).on && batch.total > 0 {
            let paid: Vec<PaymentId> = batch
                .events
                .iter()
                .filter_map(|event| match event {
                    DomainEvent::PaymentPaid { payment_id, .. } => Some(*payment_id),
                    _ => None,
                })
                .collect();
            let base = self.admin_base();
            let mut conn = self.services.store().conn().await?;
            let payments: HashMap<PaymentId, Payment> = conn
                .payments()
                .by_ids(&paid)
                .await?
                .into_iter()
                .map(|payment| (payment.id, payment))
                .collect();
            let order_ids: Vec<OrderId> = payments.values().map(|p| p.order_id).collect();
            let orders: HashMap<OrderId, Order> = conn
                .orders()
                .by_ids(&order_ids)
                .await?
                .into_iter()
                .map(|order| (order.id, order))
                .collect();
            let tariffs: HashMap<TariffId, String> = conn
                .tariffs()
                .list(true)
                .await?
                .into_iter()
                .map(|tariff| (tariff.id, tariff.name))
                .collect();
            let outcomes = conn.payments().outcomes(&paid).await?;
            let registry = self.services.payments();
            let currency = self.services.settings().currency;
            let mut lines = vec![format!("<b>Новые оплаты: {}</b>", batch.total)];
            let mut shown: Vec<ClientId> = Vec::new();
            for id in &paid {
                let Some(payment) = payments.get(id) else {
                    continue;
                };
                let name = escape(&cut(
                    &client_label(&mut conn, payment.client_id.0).await,
                    NAME_MAX,
                ));
                let method = escape(&method_title(registry, currency, payment));
                let via = match registry
                    .get(payment.provider.as_str())
                    .map(|found| escape(found.title()))
                {
                    Some(title) if method.starts_with(&title) => method,
                    Some(title) if payment.pay_method.is_some() => format!("{title}, {method}"),
                    Some(title) => title,
                    None => method,
                };
                let mut parts = vec![amount_text(payment.amount)];
                if let Some(order) = orders.get(&payment.order_id) {
                    parts.push(bought_text(&order_brief(order, &tariffs)));
                }
                parts.push(client_ref(base.as_deref(), payment.client_id, &name));
                parts.push(via);
                let outcome = outcomes.get(id).copied().unwrap_or_default();
                parts.extend(
                    marks(payment, outcome.refunded_minor, outcome.repeat)
                        .into_iter()
                        .map(str::to_string),
                );
                lines.push(parts.join(" · "));
                shown.push(payment.client_id);
            }
            let rest = batch.total.saturating_sub(shown.len());
            if rest > 0 {
                lines.push(format!("и ещё {rest}"));
            }
            out.push(Alert {
                kind: AlertKind::Sales,
                html: lines.join("\n"),
                path: match shown.as_slice() {
                    [only] if batch.total == 1 => format!("/admin/clients/{}", only.0),
                    _ => "/admin/money".to_string(),
                },
            });
        }
        self.put_state(SALES_CURSOR, &serde_json::json!(batch.newest))
            .await?;
        Ok(out)
    }
}
