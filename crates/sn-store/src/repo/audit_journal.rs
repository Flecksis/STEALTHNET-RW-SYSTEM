use super::client_history::ClientPerson;
use super::clients::ClientsRepo;
use crate::error::StoreResult;
use chrono::{DateTime, Utc};
use sn_domain::ids::ClientId;
use sqlx::{PgConnection, Postgres, QueryBuilder};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditActorFilter {
    Admin(i64),
    ApiKey(i64),
    Module(String),
    System,
    Clients,
    Client(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditSearch {
    Ip(String),
    Text {
        pattern: String,
        actions: Vec<String>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditFilter {
    pub actor: Option<AuditActorFilter>,
    pub actions: Option<Vec<String>>,
    pub client: Option<i64>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub search: Option<AuditSearch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditCursor {
    pub at: DateTime<Utc>,
    pub id: i64,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct AuditRow {
    pub id: i64,
    pub actor_kind: String,
    pub actor_id: Option<i64>,
    pub action: String,
    pub entity_kind: Option<String>,
    pub entity_id: Option<i64>,
    pub details: serde_json::Value,
    pub ip: Option<String>,
    pub created_at: DateTime<Utc>,
    pub api_key_id: Option<i64>,
    pub ext_module: Option<String>,
    pub via_recorded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AuditAdminRef {
    pub id: i64,
    pub email: String,
    pub role: String,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AuditKeyRef {
    pub id: i64,
    pub name: String,
    pub owner: Option<String>,
    pub revoked: bool,
    pub module: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AuditModuleRef {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AuditSubscriptionRef {
    pub id: i64,
    pub client_id: i64,
    pub tariff: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AuditPaymentRef {
    pub id: i64,
    pub client_id: i64,
    pub amount_minor: i64,
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AuditTicketRef {
    pub id: i64,
    pub client_id: i64,
    pub subject: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AuditRefs {
    pub admins: HashMap<i64, AuditAdminRef>,
    pub keys: HashMap<i64, AuditKeyRef>,
    pub modules: HashMap<String, String>,
    pub clients: HashMap<i64, ClientPerson>,
    pub subscriptions: HashMap<i64, AuditSubscriptionRef>,
    pub orders: HashMap<i64, i64>,
    pub payments: HashMap<i64, AuditPaymentRef>,
    pub tickets: HashMap<i64, AuditTicketRef>,
    pub tariffs: HashMap<i64, String>,
    pub categories: HashMap<i64, String>,
}

const ROW_COLUMNS: &str = "a.id, a.actor_kind, a.actor_id, a.action, a.entity_kind, a.entity_id, \
     a.details, a.ip, a.created_at, a.api_key_id, a.ext_module, a.via_recorded";

pub fn like_pattern(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('%');
    for c in text.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

fn ids_of<'r>(rows: &'r [AuditRow], kind: &'r str) -> impl Iterator<Item = i64> + 'r {
    rows.iter()
        .filter(move |row| row.entity_kind.as_deref() == Some(kind))
        .filter_map(|row| row.entity_id)
}

fn unique(mut ids: Vec<i64>) -> Vec<i64> {
    ids.sort_unstable();
    ids.dedup();
    ids
}

pub struct AuditJournalRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> AuditJournalRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        AuditJournalRepo { conn }
    }

    pub async fn page(
        &mut self,
        filter: &AuditFilter,
        after: Option<AuditCursor>,
        limit: i64,
    ) -> StoreResult<Vec<AuditRow>> {
        let mut q: QueryBuilder<Postgres> =
            QueryBuilder::new(format!("SELECT {ROW_COLUMNS} FROM audit_log a WHERE TRUE"));
        match &filter.actor {
            Some(AuditActorFilter::Admin(id)) => {
                q.push(" AND a.actor_kind = 'admin' AND a.actor_id = ")
                    .push_bind(*id)
                    .push(" AND a.api_key_id IS NULL");
            }
            Some(AuditActorFilter::ApiKey(id)) => {
                q.push(" AND a.api_key_id = ").push_bind(*id);
            }
            Some(AuditActorFilter::Module(id)) => {
                q.push(" AND a.ext_module = ").push_bind(id.clone());
            }
            Some(AuditActorFilter::System) => {
                q.push(" AND a.actor_kind IN ('system', 'bot') AND a.api_key_id IS NULL");
            }
            Some(AuditActorFilter::Clients) => {
                q.push(" AND a.actor_kind = 'client'");
            }
            Some(AuditActorFilter::Client(id)) => {
                q.push(" AND a.actor_kind = 'client' AND a.actor_id = ")
                    .push_bind(*id);
            }
            None => {}
        }
        if let Some(actions) = &filter.actions {
            q.push(" AND a.action = ANY(")
                .push_bind(actions.clone())
                .push(")");
        }
        if let Some(client) = filter.client {
            q.push(
                " AND a.id IN (SELECT x.id FROM audit_log x WHERE (x.entity_kind, x.entity_id) IN (\
                 SELECT 'client', ",
            )
            .push_bind(client)
            .push("::bigint UNION ALL SELECT 'subscription', s.id FROM subscriptions s WHERE s.client_id = ")
            .push_bind(client)
            .push(" UNION ALL SELECT 'order', o.id FROM orders o WHERE o.client_id = ")
            .push_bind(client)
            .push(" UNION ALL SELECT 'payment', p.id FROM payments p WHERE p.client_id = ")
            .push_bind(client)
            .push(" UNION ALL SELECT 'ticket', t.id FROM tickets t WHERE t.client_id = ")
            .push_bind(client)
            .push(") UNION ALL SELECT x.id FROM audit_log x WHERE x.actor_kind = 'client' AND x.actor_id = ")
            .push_bind(client)
            .push(")");
        }
        match &filter.search {
            Some(AuditSearch::Ip(ip)) => {
                q.push(" AND a.ip = ").push_bind(ip.clone());
            }
            Some(AuditSearch::Text { pattern, actions }) => {
                q.push(" AND (a.details::text ILIKE ")
                    .push_bind(pattern.clone());
                if !actions.is_empty() {
                    q.push(" OR a.action = ANY(")
                        .push_bind(actions.clone())
                        .push(")");
                }
                q.push(")");
            }
            None => {}
        }
        if let Some(since) = filter.since {
            q.push(" AND a.created_at >= ").push_bind(since);
        }
        if let Some(until) = filter.until {
            q.push(" AND a.created_at < ").push_bind(until);
        }
        if let Some(cursor) = after {
            q.push(" AND (a.created_at, a.id) < (")
                .push_bind(cursor.at)
                .push(", ")
                .push_bind(cursor.id)
                .push(")");
        }
        q.push(" ORDER BY a.created_at DESC, a.id DESC LIMIT ")
            .push_bind(limit);
        Ok(q.build_query_as::<AuditRow>()
            .persistent(false)
            .fetch_all(&mut *self.conn)
            .await?)
    }

    pub async fn entry(&mut self, id: i64) -> StoreResult<Option<AuditRow>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {ROW_COLUMNS} FROM audit_log a WHERE a.id = $1"
        ))
        .bind(id)
        .fetch_optional(&mut *self.conn)
        .await?)
    }

    pub async fn admins(&mut self) -> StoreResult<Vec<AuditAdminRef>> {
        Ok(sqlx::query_as(
            "SELECT id, email, role, disabled_at IS NOT NULL AS disabled FROM admins ORDER BY id",
        )
        .fetch_all(&mut *self.conn)
        .await?)
    }

    pub async fn keys(&mut self) -> StoreResult<Vec<AuditKeyRef>> {
        Ok(sqlx::query_as(
            "SELECT k.id, k.name, a.email AS owner, k.revoked_at IS NOT NULL AS revoked, \
             COALESCE(m.id, (SELECT l.ext_module FROM audit_log l WHERE l.api_key_id = k.id \
                 AND l.ext_module IS NOT NULL ORDER BY l.id DESC LIMIT 1)) AS module \
             FROM api_keys k LEFT JOIN admins a ON a.id = k.created_by \
             LEFT JOIN ext_modules m ON m.api_key_id = k.id ORDER BY k.id",
        )
        .fetch_all(&mut *self.conn)
        .await?)
    }

    pub async fn modules(&mut self) -> StoreResult<Vec<AuditModuleRef>> {
        Ok(sqlx::query_as(
            "SELECT id, manifest ->> 'name' AS name FROM ext_modules \
             UNION SELECT DISTINCT l.ext_module, NULL FROM audit_log l \
             WHERE l.ext_module IS NOT NULL \
             AND NOT EXISTS (SELECT 1 FROM ext_modules m WHERE m.id = l.ext_module) \
             ORDER BY 1",
        )
        .fetch_all(&mut *self.conn)
        .await?)
    }

    pub async fn refs(&mut self, rows: &[AuditRow]) -> StoreResult<AuditRefs> {
        let mut refs = AuditRefs::default();
        let admin_ids = unique(
            rows.iter()
                .filter(|row| row.actor_kind == "admin")
                .filter_map(|row| row.actor_id)
                .chain(ids_of(rows, "admin"))
                .collect(),
        );
        if !admin_ids.is_empty() {
            let admins: Vec<AuditAdminRef> = sqlx::query_as(
                "SELECT id, email, role, disabled_at IS NOT NULL AS disabled FROM admins WHERE id = ANY($1)",
            )
            .bind(&admin_ids)
            .fetch_all(&mut *self.conn)
            .await?;
            refs.admins = admins.into_iter().map(|a| (a.id, a)).collect();
        }
        let key_ids = unique(
            rows.iter()
                .filter_map(|row| row.api_key_id)
                .chain(ids_of(rows, "api_key"))
                .collect(),
        );
        if !key_ids.is_empty() {
            let keys: Vec<AuditKeyRef> = sqlx::query_as(
                "SELECT k.id, k.name, a.email AS owner, k.revoked_at IS NOT NULL AS revoked, m.id AS module \
                 FROM api_keys k LEFT JOIN admins a ON a.id = k.created_by \
                 LEFT JOIN ext_modules m ON m.api_key_id = k.id WHERE k.id = ANY($1)",
            )
            .bind(&key_ids)
            .fetch_all(&mut *self.conn)
            .await?;
            refs.keys = keys.into_iter().map(|k| (k.id, k)).collect();
        }
        let mut module_ids: Vec<String> = rows
            .iter()
            .filter_map(|row| row.ext_module.clone())
            .chain(
                rows.iter()
                    .filter(|row| row.entity_kind.as_deref() == Some("ext_module"))
                    .filter_map(|row| {
                        row.details
                            .get("module")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    }),
            )
            .collect();
        module_ids.sort();
        module_ids.dedup();
        if !module_ids.is_empty() {
            let modules: Vec<(String, Option<String>)> = sqlx::query_as(
                "SELECT id, manifest ->> 'name' FROM ext_modules WHERE id = ANY($1)",
            )
            .bind(&module_ids)
            .fetch_all(&mut *self.conn)
            .await?;
            refs.modules = modules
                .into_iter()
                .filter_map(|(id, name)| name.map(|name| (id, name)))
                .collect();
        }
        let sub_ids = unique(ids_of(rows, "subscription").collect());
        if !sub_ids.is_empty() {
            let subs: Vec<AuditSubscriptionRef> = sqlx::query_as(
                "SELECT s.id, s.client_id, t.name AS tariff FROM subscriptions s \
                 LEFT JOIN tariffs t ON t.id = s.tariff_id WHERE s.id = ANY($1)",
            )
            .bind(&sub_ids)
            .fetch_all(&mut *self.conn)
            .await?;
            refs.subscriptions = subs.into_iter().map(|s| (s.id, s)).collect();
        }
        let order_ids = unique(ids_of(rows, "order").collect());
        if !order_ids.is_empty() {
            let orders: Vec<(i64, i64)> =
                sqlx::query_as("SELECT id, client_id FROM orders WHERE id = ANY($1)")
                    .bind(&order_ids)
                    .fetch_all(&mut *self.conn)
                    .await?;
            refs.orders = orders.into_iter().collect();
        }
        let payment_ids = unique(ids_of(rows, "payment").collect());
        if !payment_ids.is_empty() {
            let payments: Vec<AuditPaymentRef> = sqlx::query_as(
                "SELECT id, client_id, amount_minor, currency FROM payments WHERE id = ANY($1)",
            )
            .bind(&payment_ids)
            .fetch_all(&mut *self.conn)
            .await?;
            refs.payments = payments.into_iter().map(|p| (p.id, p)).collect();
        }
        let ticket_ids = unique(ids_of(rows, "ticket").collect());
        if !ticket_ids.is_empty() {
            let tickets: Vec<AuditTicketRef> =
                sqlx::query_as("SELECT id, client_id, subject FROM tickets WHERE id = ANY($1)")
                    .bind(&ticket_ids)
                    .fetch_all(&mut *self.conn)
                    .await?;
            refs.tickets = tickets.into_iter().map(|t| (t.id, t)).collect();
        }
        let tariff_ids = unique(ids_of(rows, "tariff").collect());
        if !tariff_ids.is_empty() {
            let tariffs: Vec<(i64, String)> =
                sqlx::query_as("SELECT id, name FROM tariffs WHERE id = ANY($1)")
                    .bind(&tariff_ids)
                    .fetch_all(&mut *self.conn)
                    .await?;
            refs.tariffs = tariffs.into_iter().collect();
        }
        let category_ids = unique(ids_of(rows, "category").collect());
        if !category_ids.is_empty() {
            let categories: Vec<(i64, String)> =
                sqlx::query_as("SELECT id, name FROM tariff_categories WHERE id = ANY($1)")
                    .bind(&category_ids)
                    .fetch_all(&mut *self.conn)
                    .await?;
            refs.categories = categories.into_iter().collect();
        }
        let client_ids: Vec<ClientId> = unique(
            rows.iter()
                .filter(|row| row.actor_kind == "client")
                .filter_map(|row| row.actor_id)
                .chain(ids_of(rows, "client"))
                .chain(refs.subscriptions.values().map(|s| s.client_id))
                .chain(refs.orders.values().copied())
                .chain(refs.payments.values().map(|p| p.client_id))
                .chain(refs.tickets.values().map(|t| t.client_id))
                .collect(),
        )
        .into_iter()
        .map(ClientId)
        .collect();
        refs.clients = ClientsRepo::new(&mut *self.conn)
            .people(&client_ids)
            .await?
            .into_iter()
            .map(|(id, person)| (id.0, person))
            .collect();
        Ok(refs)
    }
}
