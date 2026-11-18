use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request};
use axum::middleware::Next;
use axum::response::Response;
use sn_domain::client::Locale;
use sn_domain::lang;

pub const HEADER: &str = "x-sn-lang";

pub const COOKIE: &str = "sn_lang";

pub fn from_cookie(headers: &HeaderMap) -> Option<Locale> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .and_then(|(_, value)| Locale::parse(value.trim()).ok())
}

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
        .or_else(|| from_cookie(request.headers()))
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
