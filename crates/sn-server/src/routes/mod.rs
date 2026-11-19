pub mod admin;
pub mod client;
mod client_email;
mod client_oauth;
mod client_photo;
mod public_pay;
pub mod support_files;
pub mod system;
pub mod webhooks;

use crate::extract::{api_key_scope, require_admin, require_client, with_client_address};
use crate::state::AppState;
use axum::Router;
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderName, HeaderValue, Method, Request, header};
use axum::middleware::from_fn_with_state;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

pub const REQUEST_ID: &str = "x-request-id";

fn cors(origins: &[String]) -> CorsLayer {
    let allowed: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static(REQUEST_ID),
            HeaderName::from_static(crate::lang::HEADER),
        ])
        .expose_headers([HeaderName::from_static(REQUEST_ID)])
        .allow_credentials(true)
}

pub fn request_span(request: &Request<Body>) -> tracing::Span {
    let request_id = request
        .headers()
        .get(REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.chars().take(64).collect::<String>())
        .unwrap_or_default();
    tracing::info_span!(
        "http",
        method = %request.method(),
        path = %shown_path(request.uri().path()),
        request_id = %request_id,
    )
}

pub fn shown_path(path: &str) -> std::borrow::Cow<'_, str> {
    let prefix = sn_services::bot_webhook::PATH_PREFIX;
    match path.split_once(prefix) {
        Some((head, _)) => std::borrow::Cow::Owned(format!("{head}{prefix}…")),
        None => std::borrow::Cow::Borrowed(path),
    }
}

const RESTORE_STATUS: &str = "/api/admin/backups/restore/status";

async fn restoring(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = request.uri().path();
    if !path.starts_with("/api/")
        || path == "/api/health"
        || path == "/api/version"
        || path == RESTORE_STATUS
    {
        return next.run(request).await;
    }
    match state.services.pause_gate().try_enter() {
        Some(work) => {
            let response = next.run(request).await;
            drop(work);
            response
        }
        None => {
            use axum::response::IntoResponse;
            let mut response = crate::error::ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "system.restoring",
                "Идёт восстановление из резервной копии. Панель заработает через несколько минут",
            )
            .into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
            response
        }
    }
}

pub fn router(state: AppState) -> Router {
    let ctx = state.services.module_context();
    let modules = state.services.modules().clone();
    let admin_modules = modules
        .admin_router(ctx.clone())
        .layer(from_fn_with_state(state.clone(), require_admin));
    let client_modules = modules
        .client_router(ctx.clone())
        .layer(from_fn_with_state(state.clone(), require_client));
    let public_modules = modules
        .public_router(ctx)
        .layer(from_fn_with_state(state.clone(), with_client_address));
    let request_id = HeaderName::from_static(REQUEST_ID);
    Router::new()
        .merge(system::routes())
        .merge(public_pay::routes())
        .merge(webhooks::routes())
        .merge(support_files::public_routes())
        .nest(
            "/api/admin",
            admin::routes()
                .merge(support_files::admin_routes())
                .layer(from_fn_with_state(state.clone(), api_key_scope)),
        )
        .nest(
            "/api/client",
            client::routes()
                .merge(client_photo::routes())
                .merge(client_oauth::routes())
                .merge(client_email::routes())
                .merge(support_files::client_routes()),
        )
        .nest_service("/api/admin/m", admin_modules)
        .nest_service("/api/client/m", client_modules)
        .nest_service("/api/public/m", public_modules)
        .fallback(crate::web::fallback)
        .layer(axum::middleware::from_fn(crate::error::client_facing))
        .layer(from_fn_with_state(
            state.clone(),
            crate::lang::client_language,
        ))
        .layer(from_fn_with_state(state.clone(), restoring))
        .layer(DefaultBodyLimit::max(state.config.body_limit_bytes))
        .layer(cors(&state.config.cors_origins))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(TraceLayer::new_for_http().make_span_with(request_span))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .layer(CatchPanicLayer::new())
        .with_state(state)
}
