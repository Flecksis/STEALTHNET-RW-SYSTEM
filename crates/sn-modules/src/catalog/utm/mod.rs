pub mod clicks;
pub mod links;
pub mod visit;

use crate::catalog::installation_currency;
use crate::catalog::start_param::Utm;
use crate::catalog::web::{Admin, WebError};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::module::Module;
use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_domain::client::RegistrationSource;
use sn_domain::events::DomainEvent;
use sn_domain::ids::ClientId;
use sn_domain::module::ModuleId;
use sn_domain::money::{Currency, Money, Stars};
use sn_store::{Db, StoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    Source,
    Campaign,
}

impl GroupBy {
    const fn columns(self) -> &'static str {
        match self {
            GroupBy::Source => "u.source AS source, NULL::text AS medium, NULL::text AS campaign",
            GroupBy::Campaign => "u.source AS source, u.medium AS medium, u.campaign AS campaign",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UtmRow {
    pub source: Option<String>,
    pub medium: Option<String>,
    pub campaign: Option<String>,
    pub clients: u32,
    pub paying_clients: u32,
    pub revenue: Money,
    pub stars: Stars,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkRef {
    pub id: i64,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientUtm {
    pub utm: Utm,
    pub start_param: String,
    pub registration_source: RegistrationSource,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<LinkRef>,
}

pub async fn record(
    db: &mut impl Db,
    client_id: ClientId,
    source: RegistrationSource,
    start_param: &str,
) -> StoreResult<bool> {
    let Some(utm) = Utm::parse(start_param) else {
        return Ok(false);
    };
    let done = sqlx::query(
        "INSERT INTO client_utm (client_id, source, medium, campaign, content, term, start_param, registration_source) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT (client_id) DO NOTHING",
    )
    .bind(client_id.0)
    .bind(&utm.source)
    .bind(&utm.medium)
    .bind(&utm.campaign)
    .bind(&utm.content)
    .bind(&utm.term)
    .bind(start_param.chars().take(256).collect::<String>())
    .bind(source.as_str())
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn for_client(db: &mut impl Db, client_id: ClientId) -> StoreResult<Option<ClientUtm>> {
    type Row = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        String,
        DateTime<Utc>,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT source, medium, campaign, content, term, start_param, registration_source, created_at \
         FROM client_utm WHERE client_id = $1",
    )
    .bind(client_id.0)
    .fetch_optional(db.conn())
    .await?;
    row.map(
        |(source, medium, campaign, content, term, start_param, registration, created_at)| {
            Ok(ClientUtm {
                utm: Utm {
                    source,
                    medium,
                    campaign,
                    content,
                    term,
                },
                start_param,
                registration_source: RegistrationSource::parse(&registration)?,
                created_at,
                link: None,
            })
        },
    )
    .transpose()
}

fn count(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

pub async fn report(
    db: &mut impl Db,
    currency: Currency,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    group: GroupBy,
) -> StoreResult<Vec<UtmRow>> {
    type Row = (
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
        i64,
        i64,
        i64,
    );
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "WITH cohort AS ( \
            SELECT u.client_id, {} FROM client_utm u \
            WHERE ($2::timestamptz IS NULL OR u.created_at >= $2) AND ($3::timestamptz IS NULL OR u.created_at < $3) \
         ), money AS ( \
            SELECT p.client_id, \
              sum(CASE WHEN p.currency = $1 THEN p.amount_minor - p.refunded_minor ELSE 0 END)::bigint AS revenue, \
              sum(CASE WHEN p.currency = 'XTR' THEN p.amount_minor - p.refunded_minor ELSE 0 END)::bigint AS stars \
            FROM payments p JOIN cohort c ON c.client_id = p.client_id \
            WHERE p.status IN ('paid', 'partially_refunded', 'refunded') AND p.provider <> 'balance' \
            GROUP BY p.client_id \
         ) \
         SELECT c.source, c.medium, c.campaign, count(*)::bigint, \
           count(*) FILTER (WHERE m.revenue > 0 OR m.stars > 0)::bigint, \
           COALESCE(sum(m.revenue), 0)::bigint, COALESCE(sum(m.stars), 0)::bigint \
         FROM cohort c LEFT JOIN money m ON m.client_id = c.client_id \
         GROUP BY c.source, c.medium, c.campaign ORDER BY count(*) DESC, c.source NULLS LAST LIMIT 500",
        group.columns()
    ))
    .bind(currency.code())
    .bind(from)
    .bind(to)
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(source, medium, campaign, clients, paying, revenue, stars)| UtmRow {
                source,
                medium,
                campaign,
                clients: count(clients),
                paying_clients: count(paying),
                revenue: Money::new(revenue, currency),
                stars: Stars(stars),
            },
        )
        .collect())
}

#[derive(Deserialize)]
struct ReportQuery {
    #[serde(default)]
    from: Option<DateTime<Utc>>,
    #[serde(default)]
    to: Option<DateTime<Utc>>,
    #[serde(default)]
    group: Option<GroupBy>,
}

async fn report_route(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<ReportQuery>,
) -> Result<Json<Vec<UtmRow>>, WebError> {
    admin.require(Permission::Marketing)?;
    let mut conn = ctx.store.conn().await?;
    let currency = installation_currency(&mut conn).await?;
    Ok(Json(
        report(
            &mut conn,
            currency,
            query.from,
            query.to,
            query.group.unwrap_or(GroupBy::Campaign),
        )
        .await?,
    ))
}

async fn client_route(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(client_id): Path<ClientId>,
) -> Result<Json<Option<ClientUtm>>, WebError> {
    admin.require(Permission::ClientsRead)?;
    let mut conn = ctx.store.conn().await?;
    let mut found = for_client(&mut conn, client_id).await?;
    if let Some(f) = found.as_mut()
        && let Some(link) = links::by_tags(&mut conn, &f.utm).await?
    {
        f.link = Some(LinkRef {
            id: link.id,
            title: link.title,
        });
    }
    Ok(Json(found))
}

pub struct UtmModule;

#[async_trait]
impl Module for UtmModule {
    fn id(&self) -> ModuleId {
        ModuleId::Utm
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(
            Router::new()
                .route("/report", get(report_route))
                .route("/clients/{client_id}", get(client_route))
                .route("/links", get(links::links_route).post(links::create_route))
                .route(
                    "/links/{id}",
                    axum::routing::put(links::update_route).delete(links::archive_route),
                )
                .route(
                    "/links/{id}/archive",
                    axum::routing::post(links::archive_route),
                )
                .with_state(ctx),
        )
    }

    fn public_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(visit::routes(ctx))
    }

    async fn on_event(&self, ctx: &ModuleContext, event: &DomainEvent) -> Result<(), ModuleError> {
        if let DomainEvent::ClientRegistered {
            client_id,
            source,
            start_param: Some(param),
        } = event
        {
            let mut conn = ctx.store.conn().await?;
            record(&mut conn, *client_id, *source, param).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::testing::{TestCore, client, context, order, paid_payment};
    use sn_domain::ids::{PeriodId, TariffId};
    use sn_domain::money::{Amount, PayCurrency};
    use sn_domain::order::{OrderKind, PurchaseIntent};
    use sn_domain::pricing::Quote;
    use sn_store::testing::test_store;

    fn empty_quote() -> Quote {
        Quote {
            currency: Currency::Usd,
            lines: Vec::new(),
            subtotal: Money::zero(Currency::Usd),
            discount: Money::zero(Currency::Usd),
            total: Money::zero(Currency::Usd),
            stars: None,
            days: 30,
            extra_devices: 0,
        }
    }

    #[tokio::test]
    async fn источник_пишется_при_регистрации_и_попадает_в_отчёт() {
        let Some(db) = test_store().await else {
            return;
        };
        let core = TestCore::new(&db.store);
        let ctx = context(&db.store, &core, Currency::Usd);
        let tiktok = client(&db.store).await;
        let tiktok_quiet = client(&db.store).await;
        let organic = client(&db.store).await;
        for (id, param) in [
            (tiktok, "s_tiktok__k_winter__ref_ABC"),
            (tiktok_quiet, "s_tiktok__k_winter"),
            (organic, "ref_ABC"),
        ] {
            UtmModule
                .on_event(
                    &ctx,
                    &DomainEvent::ClientRegistered {
                        client_id: id,
                        source: RegistrationSource::Bot,
                        start_param: Some(param.into()),
                    },
                )
                .await
                .unwrap();
        }
        let mut conn = db.store.conn().await.unwrap();
        let saved = for_client(&mut conn, tiktok).await.unwrap().unwrap();
        assert_eq!(saved.utm.source.as_deref(), Some("tiktok"));
        assert_eq!(saved.utm.campaign.as_deref(), Some("winter"));
        assert_eq!(saved.registration_source, RegistrationSource::Bot);
        assert!(for_client(&mut conn, organic).await.unwrap().is_none());
        drop(conn);
        let intent = PurchaseIntent::Subscription {
            tariff_id: TariffId(1),
            period_id: PeriodId(1),
            extra_devices: 0,
            target: None,
        };
        let first = order(
            &db.store,
            tiktok,
            OrderKind::New,
            intent.clone(),
            empty_quote(),
        )
        .await;
        paid_payment(
            &db.store,
            first.id,
            tiktok,
            "cryptopay",
            Amount::new(699, PayCurrency::Usd),
        )
        .await;
        let second = order(
            &db.store,
            tiktok,
            OrderKind::New,
            intent.clone(),
            empty_quote(),
        )
        .await;
        paid_payment(
            &db.store,
            second.id,
            tiktok,
            "stars",
            Amount::new(350, PayCurrency::Xtr),
        )
        .await;
        let third = order(&db.store, tiktok, OrderKind::New, intent, empty_quote()).await;
        paid_payment(
            &db.store,
            third.id,
            tiktok,
            "balance",
            Amount::new(299, PayCurrency::Usd),
        )
        .await;
        let mut conn = db.store.conn().await.unwrap();
        let rows = report(&mut conn, Currency::Usd, None, None, GroupBy::Campaign)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.source.as_deref(), Some("tiktok"));
        assert_eq!(row.campaign.as_deref(), Some("winter"));
        assert_eq!((row.clients, row.paying_clients), (2, 1));
        assert_eq!(row.revenue, Money::new(699, Currency::Usd));
        assert_eq!(row.stars, Stars(350));
        let by_source = report(&mut conn, Currency::Usd, None, None, GroupBy::Source)
            .await
            .unwrap();
        assert_eq!(by_source[0].campaign, None);
        drop(conn);
        db.cleanup().await;
    }
}
