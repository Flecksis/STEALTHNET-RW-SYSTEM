use super::{decode, json};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::module::ModuleId;
use sn_domain::settings::{Appearance, AutoRenewSettings, ConnectSettings, TrialSettings};
use sn_modules::ModuleView;
use sn_services::branding::{LogoView, SiteUpdate};
use sn_services::settings::{
    BotUpdate, CoreUpdate, ModuleConfigUpdate, ModuleConfigView, PaymentsUpdate, ProviderDetails,
    ProviderUpdate, RemnaCheck, RemnaCheckRequest, RemnaUpdate, SubscriptionSettingsUpdate,
    WebhookSecret,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings/{section}", get(read).put(write))
        .route("/settings/bot/restart", post(restart_bot))
        .route(
            "/settings/logo",
            post(upload_logo)
                .delete(delete_logo)
                .layer(DefaultBodyLimit::max(LOGO_UPLOAD_LIMIT)),
        )
        .route("/settings/remnawave/check", post(check_remnawave))
        .route("/settings/remnawave/webhook-secret", post(webhook_secret))
        .route("/settings/remnawave/page-configs", get(page_configs))
        .route("/settings/remnawave/webhook-env", get(webhook_env))
        .route("/payments/providers", get(providers))
        .route(
            "/payments/providers/{id}",
            get(provider).put(update_provider),
        )
        .route("/modules", get(modules))
        .route("/modules/{id}", patch(toggle_module))
        .route(
            "/modules/{id}/config",
            get(module_config).put(update_module_config),
        )
}

async fn read(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(section): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    let ops = state.services.settings_ops();
    let value = match section.as_str() {
        "core" => json(&ops.core())?,
        "appearance" => json(&ops.appearance())?,
        "connect" => json(&ops.connect())?,
        "payments" => json(&ops.payments())?,
        "trial" => json(&ops.trial())?,
        "autorenew" => json(&ops.autorenew())?,
        "remnawave" => json(&ops.remnawave().await?)?,
        "subscription" => json(&ops.subscription_settings().await?)?,
        "bot" => json(&ops.bot().await?)?,
        "site" => json(&state.services.branding().site())?,
        _ => return Err(ApiError::not_found()),
    };
    Ok(Json(value))
}

async fn write(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(section): Path<String>,
    ApiJson(body): ApiJson<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    let ops = state.services.settings_ops();
    let by = admin.0.admin_id;
    let value = match section.as_str() {
        "core" => json(&ops.update_core(&decode::<CoreUpdate>(body)?, by).await?)?,
        "appearance" => json(
            &ops.update_appearance(&decode::<Appearance>(body)?, by)
                .await?,
        )?,
        "connect" => json(
            &ops.update_connect(&decode::<ConnectSettings>(body)?, by)
                .await?,
        )?,
        "payments" => json(
            &ops.update_payments(&decode::<PaymentsUpdate>(body)?, by)
                .await?,
        )?,
        "trial" => json(
            &ops.update_trial(&decode::<TrialSettings>(body)?, by)
                .await?,
        )?,
        "autorenew" => json(
            &ops.update_autorenew(&decode::<AutoRenewSettings>(body)?, by)
                .await?,
        )?,
        "remnawave" => json(
            &ops.update_remnawave(&decode::<RemnaUpdate>(body)?, by)
                .await?,
        )?,
        "subscription" => json(
            &ops.update_subscription_settings(&decode::<SubscriptionSettingsUpdate>(body)?, by)
                .await?,
        )?,
        "bot" => json(&ops.update_bot(&decode::<BotUpdate>(body)?, by).await?)?,
        "site" => json(
            &state
                .services
                .branding()
                .update_site(&decode::<SiteUpdate>(body)?, by)
                .await?,
        )?,
        _ => return Err(ApiError::not_found()),
    };
    Ok(Json(value))
}

#[derive(Debug, Default, Deserialize)]
struct RestartBot {
    #[serde(default)]
    take_over: bool,
}

async fn restart_bot(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    body: Option<ApiJson<RestartBot>>,
) -> ApiResult<Json<sn_services::settings::BotSettingsView>> {
    admin.require(Permission::Settings)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    Ok(Json(
        state
            .services
            .settings_ops()
            .restart_bot(body.take_over, admin.0.admin_id)
            .await?,
    ))
}

const LOGO_UPLOAD_LIMIT: usize = 4 * 1024 * 1024;

fn image_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .is_some_and(|value| matches!(value.as_str(), "image/png" | "image/jpeg" | "image/webp"))
}

async fn upload_logo(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<LogoView>> {
    admin.require(Permission::Settings)?;
    if !image_type(&headers) {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "logo.format",
            "Подходят только PNG, JPEG и WebP",
        ));
    }
    Ok(Json(
        state
            .services
            .branding()
            .upload_logo(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn delete_logo(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    let removed = state
        .services
        .branding()
        .delete_logo(admin.0.admin_id)
        .await?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}

async fn webhook_env(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        serde_json::json!({ "env": state.services.settings_ops().webhook_env_full() }),
    ))
}

async fn page_configs(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<sn_services::settings::PageConfig>>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.settings_ops().page_configs().await?))
}

async fn check_remnawave(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    body: Option<ApiJson<RemnaCheckRequest>>,
) -> ApiResult<Json<RemnaCheck>> {
    admin.require(Permission::Settings)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    Ok(Json(
        state.services.settings_ops().check_remnawave(&body).await?,
    ))
}

async fn webhook_secret(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<WebhookSecret>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .settings_ops()
            .generate_remna_webhook_secret(admin.0.admin_id)
            .await?,
    ))
}

async fn providers(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    let payments = state.services.settings_ops().payments();
    Ok(Json(serde_json::json!({
        "currency": payments.currency,
        "order": payments.order,
        "providers": payments.providers,
    })))
}

async fn provider(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ProviderDetails>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.settings_ops().provider(&id)?))
}

async fn update_provider(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ProviderUpdate>,
) -> ApiResult<Json<ProviderDetails>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .settings_ops()
            .update_provider(&id, &body, admin.0.admin_id)
            .await?,
    ))
}

async fn modules(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Modules)?;
    Ok(Json(serde_json::json!({
        "modules": state.services.modules().views(),
        "core": sn_domain::module::CoreFeature::ALL.map(|f| f.title_ru()),
    })))
}

fn module_id(id: &str) -> ApiResult<ModuleId> {
    ModuleId::parse(id).map_err(|_| ApiError::not_found())
}

#[derive(Deserialize)]
struct ToggleRequest {
    enabled: bool,
}

async fn toggle_module(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ToggleRequest>,
) -> ApiResult<Json<ModuleView>> {
    admin.require(Permission::Modules)?;
    Ok(Json(
        state
            .services
            .settings_ops()
            .toggle_module(module_id(&id)?, body.enabled, admin.0.admin_id)
            .await?,
    ))
}

async fn module_config(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ModuleConfigView>> {
    admin.require(Permission::Modules)?;
    Ok(Json(
        state.services.settings_ops().module_config(module_id(&id)?),
    ))
}

async fn update_module_config(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ModuleConfigUpdate>,
) -> ApiResult<Json<ModuleConfigView>> {
    admin.require(Permission::Modules)?;
    Ok(Json(
        state
            .services
            .settings_ops()
            .update_module_config(module_id(&id)?, &body, admin.0.admin_id)
            .await?,
    ))
}
