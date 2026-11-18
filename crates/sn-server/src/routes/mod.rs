pub mod admin;
pub mod client;
mod client_oauth;
mod client_photo;
mod public_pay;
pub mod system;
pub mod webhooks;

use crate::extract::{require_admin, require_client};
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
        path = %request.uri().path(),
        request_id = %request_id,
    )
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
    let public_modules = modules.public_router(ctx);
    let request_id = HeaderName::from_static(REQUEST_ID);
    Router::new()
        .merge(system::routes())
        .merge(public_pay::routes())
        .merge(webhooks::routes())
        .nest("/api/admin", admin::routes())
        .nest(
            "/api/client",
            client::routes()
                .merge(client_photo::routes())
                .merge(client_oauth::routes()),
        )
        .nest_service("/api/admin/m", admin_modules)
        .nest_service("/api/client/m", client_modules)
        .nest_service("/api/public/m", public_modules)
        .fallback(crate::web::fallback)
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
