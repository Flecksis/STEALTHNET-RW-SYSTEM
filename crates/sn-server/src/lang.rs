use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use sn_domain::client::Locale;
use sn_domain::lang;

pub const HEADER: &str = "x-sn-lang";

fn wanted(request: &Request<Body>) -> Option<Locale> {
    let path = request.uri().path();
    if !(path.starts_with("/api/client") || path.starts_with("/api/public")) {
        return None;
    }
    request
        .headers()
        .get(HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Locale::parse(value.trim()).ok())
}

pub async fn client_language(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    match wanted(&request) {
        Some(locale) => {
            let locale = state.services.locale_of(locale);
            lang::scope(locale, next.run(request)).await
        }
        None => next.run(request).await,
    }
}
