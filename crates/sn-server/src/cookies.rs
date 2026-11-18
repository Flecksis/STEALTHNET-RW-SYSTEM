use crate::state::AppState;
use axum::http::HeaderValue;
use axum::http::header::SET_COOKIE;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

impl SameSite {
    const fn as_str(self) -> &'static str {
        match self {
            SameSite::Strict => "Strict",
            SameSite::Lax => "Lax",
            SameSite::None => "None",
        }
    }
}

pub fn session(
    state: &AppState,
    name: &str,
    value: &str,
    expires_at: Option<DateTime<Utc>>,
    same_site: SameSite,
) -> Option<HeaderValue> {
    let max_age = expires_at
        .map(|at| (at - state.services.now()).num_seconds().max(0))
        .unwrap_or(0);
    let secure = state.config.secure_cookies() || same_site == SameSite::None;
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{name}={value}; Path=/api; HttpOnly; SameSite={}; Max-Age={max_age}{secure}",
        same_site.as_str()
    ))
    .ok()
}

pub fn scoped(
    state: &AppState,
    name: &str,
    value: &str,
    path: &str,
    max_age: i64,
    same_site: SameSite,
) -> Option<HeaderValue> {
    let secure = state.config.secure_cookies() || same_site == SameSite::None;
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{name}={value}; Path={path}; HttpOnly; SameSite={}; Max-Age={}{secure}",
        same_site.as_str(),
        max_age.max(0)
    ))
    .ok()
}

pub fn with_cookie(cookie: Option<HeaderValue>, body: impl IntoResponse) -> Response {
    let mut response = body.into_response();
    if let Some(cookie) = cookie {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}
