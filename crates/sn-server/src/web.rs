use crate::error::ApiError;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, Method, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::path::Path;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const REVALIDATE: &str = "no-cache";

fn is_api(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

fn embeddable(path: &str) -> bool {
    path == "/app" || path.starts_with("/app/")
}

fn hashed_asset(path: &str) -> bool {
    path.starts_with("/assets/")
}

pub async fn fallback(State(state): State<AppState>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_string();
    if is_api(&path) {
        return ApiError::not_found().into_response();
    }
    let Some(dir) = state.config.web_dir.as_deref() else {
        return ApiError::not_found().into_response();
    };
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    serve(dir, &path, request).await
}

async fn serve(dir: &Path, path: &str, request: Request<Body>) -> Response {
    let index = dir.join("index.html");
    let service = ServeDir::new(dir)
        .precompressed_br()
        .precompressed_gzip()
        .fallback(ServeFile::new(index));
    let mut response = match service.oneshot(request).await {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    };
    let cache = if hashed_asset(path) && response.status() == StatusCode::OK {
        IMMUTABLE
    } else {
        REVALIDATE
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if !embeddable(path) {
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn адреса_api_не_уходят_в_интерфейс() {
        assert!(is_api("/api"));
        assert!(is_api("/api/admin/me"));
        assert!(!is_api("/apiary"));
        assert!(!is_api("/admin"));
    }

    #[test]
    fn во_фрейме_открывается_только_мини_приложение() {
        assert!(embeddable("/app"));
        assert!(embeddable("/app/plans"));
        assert!(!embeddable("/admin"));
        assert!(!embeddable("/application"));
    }
}
