use super::store::{self, AdminWithdrawal, Referrer, Totals, Withdrawal, WithdrawalFilter};
use super::{Decision, LEVELS, ReferralError, ReferralOverview, WithdrawalRequest};
use crate::catalog::installation_currency;
use crate::catalog::web::{Admin, Customer, WebError};
use crate::context::ModuleContext;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_domain::ids::ClientId;
use sn_domain::ledger::Wallet;
use sn_domain::money::Money;
use sn_domain::paging::{Page, PageRequest};
use sn_store::Db;

impl From<ReferralError> for WebError {
    fn from(err: ReferralError) -> Self {
        match err {
            ReferralError::Rejected(rejection) => {
                WebError::rejected(rejection.code(), rejection.message())
            }
            ReferralError::Module(err) => err.into(),
        }
    }
}

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/summary", get(summary))
        .route("/referrers", get(referrers))
        .route("/withdrawals", get(withdrawals))
        .route("/withdrawals/{id}/decision", post(decide))
        .route("/clients/{client_id}", get(client_stats))
        .route("/clients/{client_id}/percent", put(set_percent))
        .with_state(ctx)
}

pub fn client(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/me", get(me))
        .route("/withdrawals", post(request_withdrawal))
        .route("/withdrawals/{id}/cancel", post(cancel))
        .with_state(ctx)
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

#[derive(Serialize)]
struct Summary {
    levels: [u8; LEVELS],
    withdraw_min: Option<Money>,
    withdrawals_enabled: bool,
    #[serde(flatten)]
    totals: Totals,
}

async fn summary(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<Summary>, WebError> {
    admin.require(Permission::Money)?;
    let mut conn = ctx.store.conn().await?;
    let currency = installation_currency(&mut conn).await?;
    let settings = super::settings(&mut conn, currency).await?;
    Ok(Json(Summary {
        levels: settings.levels,
        withdraw_min: settings.withdraw_min,
        withdrawals_enabled: settings.withdrawals,
        totals: store::totals(&mut conn, currency).await?,
    }))
}

#[derive(Deserialize)]
struct PageQuery {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

async fn referrers(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<Referrer>>, WebError> {
    admin.require(Permission::Money)?;
    let mut conn = ctx.store.conn().await?;
    let currency = installation_currency(&mut conn).await?;
    Ok(Json(
        store::referrers(
            &mut conn,
            currency,
            PageRequest::new(query.limit, query.after),
        )
        .await?,
    ))
}

async fn withdrawals(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<AdminWithdrawal>>, WebError> {
    admin.require(Permission::Money)?;
    let filter = WithdrawalFilter::parse(query.status.as_deref()).ok_or_else(|| {
        WebError::rejected(
            "validation.invalid_value",
            "фильтр: pending, decided, paid, rejected, canceled или all",
        )
    })?;
    let mut conn = ctx.store.conn().await?;
    Ok(Json(
        store::list_withdrawals(
            &mut conn,
            filter,
            PageRequest::new(query.limit, query.after),
        )
        .await?,
    ))
}

async fn decide(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(decision): Json<Decision>,
) -> Result<Json<Withdrawal>, WebError> {
    admin.require(Permission::Money)?;
    Ok(Json(
        super::decide(&ctx, id, &decision, Some(admin.0.admin_id)).await?,
    ))
}

#[derive(Serialize)]
struct ClientStats {
    code: Option<String>,
    personal_percent: Option<u8>,
    referrer_id: Option<ClientId>,
    invited: u32,
    earned: Money,
    balance: Money,
}

async fn client_stats(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(client_id): Path<ClientId>,
) -> Result<Json<ClientStats>, WebError> {
    admin.require(Permission::Money)?;
    let mut conn = ctx.store.conn().await?;
    conn.clients().get(client_id).await?;
    let currency = installation_currency(&mut conn).await?;
    Ok(Json(ClientStats {
        code: store::code_of(&mut conn, client_id).await?,
        personal_percent: store::personal_percent(&mut conn, client_id).await?,
        referrer_id: store::referrer_of(&mut conn, client_id).await?,
        invited: store::invited_count(&mut conn, client_id).await?,
        earned: store::earned(&mut conn, client_id, currency).await?,
        balance: conn
            .ledger()
            .balance(client_id, Wallet::Referral, currency)
            .await?,
    }))
}

#[derive(Deserialize)]
struct PercentBody {
    percent: Option<u8>,
}

async fn set_percent(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(client_id): Path<ClientId>,
    Json(body): Json<PercentBody>,
) -> Result<Json<serde_json::Value>, WebError> {
    admin.require(Permission::Marketing)?;
    if body.percent.is_some_and(|p| p > 100) {
        return Err(WebError::rejected(
            "validation.invalid_value",
            "процент от 0 до 100",
        ));
    }
    let mut conn = ctx.store.conn().await?;
    conn.clients().get(client_id).await?;
    super::ensure_code(&mut conn, client_id).await?;
    store::set_personal_percent(&mut conn, client_id, body.percent).await?;
    Ok(Json(
        serde_json::json!({ "personal_percent": body.percent }),
    ))
}

async fn me(
    State(ctx): State<ModuleContext>,
    customer: Customer,
) -> Result<Json<ReferralOverview>, WebError> {
    Ok(Json(super::overview(&ctx, customer.0.client_id).await?))
}

async fn request_withdrawal(
    State(ctx): State<ModuleContext>,
    customer: Customer,
    Json(body): Json<WithdrawalRequest>,
) -> Result<Json<Withdrawal>, WebError> {
    customer.require_full()?;
    Ok(Json(
        super::request_withdrawal(&ctx, customer.0.client_id, &body).await?,
    ))
}

async fn cancel(
    State(ctx): State<ModuleContext>,
    customer: Customer,
    Path(id): Path<i64>,
) -> Result<Json<Withdrawal>, WebError> {
    customer.require_full()?;
    Ok(Json(
        super::cancel_withdrawal(&ctx, customer.0.client_id, id).await?,
    ))
}
