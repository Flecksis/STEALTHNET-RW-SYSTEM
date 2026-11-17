use super::{
    Activation, ListFilter, NewPromo, PromoCode, PromoError, PromoListItem, PromoRejection, Purpose,
};
use crate::catalog::installation_currency;
use crate::catalog::web::{Admin, Customer, WebError};
use crate::context::ModuleContext;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_domain::ids::{SubscriptionId, TariffId};
use sn_domain::paging::{Page, PageRequest};

impl From<PromoError> for WebError {
    fn from(err: PromoError) -> Self {
        match err {
            PromoError::Rejected(PromoRejection::TooManyAttempts) => WebError::new(
                StatusCode::TOO_MANY_REQUESTS,
                PromoRejection::TooManyAttempts.code(),
                PromoRejection::TooManyAttempts.message(),
            ),
            PromoError::Rejected(rejection) => {
                WebError::rejected(rejection.code(), rejection.message())
            }
            PromoError::Module(err) => err.into(),
        }
    }
}

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/codes", get(list).post(create))
        .route("/codes/{id}/disable", post(disable))
        .route("/codes/{id}/enable", post(enable))
        .with_state(ctx)
}

pub fn client(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/check", post(check))
        .route("/activate", post(activate))
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

async fn list(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<PromoListItem>>, WebError> {
    admin.require(Permission::Marketing)?;
    let filter = ListFilter::parse(query.status.as_deref()).ok_or_else(|| {
        WebError::rejected(
            "validation.invalid_value",
            "фильтр: active, disabled или all",
        )
    })?;
    Ok(Json(
        super::list(&ctx, filter, PageRequest::new(query.limit, query.after)).await?,
    ))
}

async fn create(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Json(body): Json<NewPromo>,
) -> Result<Json<PromoCode>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(
        super::create(&ctx, &body, Some(admin.0.admin_id)).await?,
    ))
}

async fn disable(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<PromoCode>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::set_active(&ctx, id, false).await?))
}

async fn enable(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<PromoCode>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::set_active(&ctx, id, true).await?))
}

#[derive(Deserialize)]
struct CheckRequest {
    code: String,
    #[serde(default)]
    tariff_id: Option<TariffId>,
}

#[derive(Serialize)]
struct CheckResponse {
    code: String,
    label: String,
    discount: bool,
    #[serde(flatten)]
    promo: super::PromoKind,
}

async fn check(
    State(ctx): State<ModuleContext>,
    customer: Customer,
    Json(body): Json<CheckRequest>,
) -> Result<Json<CheckResponse>, WebError> {
    let currency = {
        let mut conn = ctx.store.conn().await?;
        installation_currency(&mut conn).await?
    };
    let promo = super::check(
        &ctx,
        customer.0.client_id,
        &body.code,
        Purpose::Check {
            tariff: body.tariff_id,
            currency,
        },
    )
    .await?;
    Ok(Json(CheckResponse {
        label: promo.label(),
        discount: promo.is_discount(),
        code: promo.code,
        promo: promo.kind,
    }))
}

#[derive(Deserialize)]
struct ActivateRequest {
    code: String,
    subscription_id: SubscriptionId,
}

async fn activate(
    State(ctx): State<ModuleContext>,
    customer: Customer,
    Json(body): Json<ActivateRequest>,
) -> Result<Json<Activation>, WebError> {
    customer.require_full()?;
    Ok(Json(
        super::activate(&ctx, customer.0.client_id, &body.code, body.subscription_id).await?,
    ))
}
