use super::clicks::{ClickChannel, record_click};
use crate::catalog::start_param::Utm;
use crate::catalog::web::{ClientAddr, WebError};
use crate::context::{ModuleContext, RateLimit};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;

pub const VISITS_PER_NETWORK: RateLimit = RateLimit {
    scope: "utm_site_visit_ip",
    limit: 60,
    window_seconds: 600,
};

#[derive(Debug, Default, Deserialize)]
pub struct Visit {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub medium: Option<String>,
    #[serde(default)]
    pub campaign: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub term: Option<String>,
}

impl Visit {
    pub fn utm(&self) -> Option<Utm> {
        Utm::from_parts(
            self.source.as_deref(),
            self.medium.as_deref(),
            self.campaign.as_deref(),
            self.content.as_deref(),
            self.term.as_deref(),
        )
    }
}

async fn visit(
    State(ctx): State<ModuleContext>,
    address: ClientAddr,
    Json(body): Json<Visit>,
) -> Result<StatusCode, WebError> {
    let Some(utm) = body.utm() else {
        return Err(WebError::rejected(
            "utm.invalid",
            "Нет ни одной метки: значение — латинские буквы, цифры, дефис и подчёркивание, до 64 символов",
        ));
    };
    if let Some(network) = address.limit_network()
        && !ctx
            .core
            .within_rate_limit(VISITS_PER_NETWORK, &network)
            .await?
    {
        return Err(WebError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "utm.rate_limited",
            "Слишком много переходов с одного адреса, попробуйте позже",
        ));
    }
    let mut conn = ctx.store.conn().await?;
    record_click(&mut conn, ClickChannel::Site, &utm, ctx.core.now()).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes(ctx: ModuleContext) -> Router {
    Router::new().route("/visit", post(visit)).with_state(ctx)
}
