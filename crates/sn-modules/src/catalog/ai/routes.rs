use super::groq::{Groq, MODELS};
use super::store::{self, LastError, Usage};
use super::{Settings, day_start};
use crate::catalog::web::{Admin, WebError};
use crate::context::ModuleContext;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use sn_domain::access::Permission;
use sn_domain::module::ModuleId;

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/overview", get(overview))
        .route("/check", post(check))
        .with_state(ctx)
}

#[derive(Serialize)]
struct Overview {
    ready: bool,
    model: String,
    today: Usage,
    daily_tokens: i64,
    client_daily: i64,
    last_error: Option<LastError>,
}

async fn overview(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<Overview>, WebError> {
    admin.require(Permission::Modules)?;
    let settings = Settings::from_config(&ctx.core.module_config(ModuleId::Ai));
    let since = day_start(ctx.core.now(), ctx.core.core_settings().remna_offset());
    let mut conn = ctx.store.conn().await?;
    Ok(Json(Overview {
        ready: settings.ready(),
        model: settings.model.clone(),
        today: store::usage(&mut conn, since).await?,
        daily_tokens: settings.daily_tokens,
        client_daily: settings.client_daily,
        last_error: store::last_error(&mut conn).await?,
    }))
}

#[derive(Serialize)]
struct Check {
    ok: bool,
    model: String,
    model_found: bool,
    available: Vec<String>,
    error: Option<String>,
}

async fn check(State(ctx): State<ModuleContext>, admin: Admin) -> Result<Json<Check>, WebError> {
    admin.require(Permission::Modules)?;
    let settings = Settings::from_config(&ctx.core.module_config(ModuleId::Ai));
    if !settings.ready() {
        return Err(WebError::rejected(
            "ai.no_key",
            "Сначала укажите API-ключ Groq и сохраните настройки",
        ));
    }
    let groq = Groq::new(&settings.api_url, &settings.api_key);
    Ok(Json(match groq.models().await {
        Ok(models) => Check {
            ok: true,
            model_found: models.contains(&settings.model),
            available: MODELS
                .iter()
                .filter(|m| models.iter().any(|id| id == *m))
                .map(|m| (*m).to_string())
                .collect(),
            model: settings.model,
            error: None,
        },
        Err(err) => Check {
            ok: false,
            model_found: false,
            available: Vec::new(),
            model: settings.model,
            error: Some(err.to_string()),
        },
    }))
}
