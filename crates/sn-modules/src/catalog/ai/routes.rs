use super::llm::{self, Llm, ModelOption, shortlist};
use super::provider::Provider;
use super::store::{self, LastError, Usage};
use super::{Settings, day_start};
use crate::catalog::web::{Admin, WebError};
use crate::context::ModuleContext;
use crate::module::ModuleConfig;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_domain::module::ModuleId;

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/overview", get(overview))
        .with_state(ctx)
}

pub fn setup(ctx: ModuleContext) -> Router {
    Router::new().route("/check", post(check)).with_state(ctx)
}

#[derive(Serialize)]
struct Overview {
    ready: bool,
    provider: &'static str,
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
        provider: settings.provider.label(),
        model: settings.model.clone(),
        today: store::usage(&mut conn, since).await?,
        daily_tokens: settings.daily_tokens,
        client_daily: settings.client_daily,
        last_error: store::last_error(&mut conn).await?,
    }))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct CheckRequest {
    pub provider: Option<String>,
    pub api_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
}

#[derive(Serialize)]
struct Check {
    ok: bool,
    provider: &'static str,
    model: String,
    model_found: bool,
    models: Vec<ModelOption>,
    error: Option<String>,
}

fn filled(value: Option<&String>) -> Option<&str> {
    value.map(|v| v.trim()).filter(|v| !v.is_empty())
}

pub(super) fn form_settings(saved: &ModuleConfig, request: &CheckRequest) -> Settings {
    let mut config = saved.clone();
    for (key, value) in [
        ("provider", &request.provider),
        ("api_url", &request.api_url),
        ("model", &request.model),
    ] {
        if let Some(value) = value {
            config.0.insert(key.into(), serde_json::json!(value.trim()));
        }
    }
    Settings::from_config(&config)
}

pub(super) fn check_key(
    saved: &Settings,
    form: &Settings,
    typed: Option<&str>,
) -> Result<String, (&'static str, &'static str)> {
    match typed {
        Some(key) => Ok(key.to_string()),
        None if saved.api_key.trim().is_empty() => Err(("ai.no_key", "Сначала вставьте API-ключ")),
        None if !saved.same_address(form) => Err((
            "ai.key_needed",
            "Для нового провайдера или адреса API вставьте ключ: сохранённый ключ туда не отправляется",
        )),
        None => Ok(saved.api_key.clone()),
    }
}

async fn check(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Json(request): Json<CheckRequest>,
) -> Result<Json<Check>, WebError> {
    admin.require(Permission::Modules)?;
    let config = ctx.core.module_config(ModuleId::Ai);
    let saved = Settings::from_config(&config);
    let form = form_settings(&config, &request);
    if request
        .provider
        .as_deref()
        .is_some_and(|p| Provider::parse(p).is_none())
    {
        return Err(WebError::rejected(
            "ai.provider",
            "Выберите провайдера из списка",
        ));
    }
    let key = check_key(&saved, &form, filled(request.api_key.as_ref()))
        .map_err(|(code, text)| WebError::rejected(code, text))?;
    if form.api_url.is_empty() {
        return Err(WebError::rejected("ai.no_url", "Укажите адрес API"));
    }
    if !saved.same_address(&form)
        && let Err(problem) = llm::check_address(&form.api_url, true)
    {
        return Err(WebError::rejected(
            "ai.bad_url",
            format!("Адрес API не подходит: {problem}"),
        ));
    }
    let provider = form.provider.label();
    let llm = match Llm::new(form.provider, &form.api_url, &key) {
        Ok(llm) => llm,
        Err(err) => {
            return Ok(Json(Check {
                ok: false,
                provider,
                model: form.model,
                model_found: false,
                models: Vec::new(),
                error: Some(err.to_string()),
            }));
        }
    };
    Ok(Json(match llm.models().await {
        Ok(models) => Check {
            ok: true,
            provider,
            model_found: models.iter().any(|m| m.id == form.model),
            models: shortlist(models),
            model: form.model,
            error: None,
        },
        Err(err) => Check {
            ok: false,
            provider,
            model: form.model,
            model_found: false,
            models: Vec::new(),
            error: Some(err.to_string()),
        },
    }))
}
