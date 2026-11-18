use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiQuery, CurrentAdmin, CurrentClient};
use crate::state::AppState;
use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::ids::TicketFileId;
use sn_domain::support::{FileKind, TicketFile};
use sn_modules::catalog::ticket_files::{MAX_MB, MB};
use sn_services::support::FileDownload;

const UPLOAD_LIMIT: usize = (MAX_MB * MB) as usize + 64 * 1024;
const FILE_POLICY: &str = "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; sandbox";
const FILE_CACHE: &str = "private, max-age=600";
const PUBLIC_PATH: &str = "/api/public/support-files";

#[derive(Deserialize)]
struct UploadQuery {
    #[serde(default)]
    name: Option<String>,
}

const fn attr_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
        )
}

pub fn disposition(kind: FileKind, name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = name
        .bytes()
        .map(|b| {
            if attr_char(b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let mode = match kind {
        FileKind::Image => "inline",
        FileKind::File => "attachment",
    };
    format!("{mode}; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

pub fn file_response(file: FileDownload) -> Response {
    let content_type = if file.mime == "text/plain" {
        "text/plain; charset=utf-8".to_string()
    } else {
        file.mime.clone()
    };
    let headers = [
        (
            header::CONTENT_TYPE,
            HeaderValue::from_str(&content_type)
                .unwrap_or(HeaderValue::from_static("application/octet-stream")),
        ),
        (
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&disposition(file.kind, &file.name))
                .unwrap_or(HeaderValue::from_static("attachment")),
        ),
        (
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
        (
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(FILE_POLICY),
        ),
        (header::CACHE_CONTROL, HeaderValue::from_static(FILE_CACHE)),
    ];
    (headers, file.bytes).into_response()
}

fn upload_name(query: &UploadQuery) -> String {
    query
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("file")
        .chars()
        .take(255)
        .collect()
}

pub fn client_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/support/files",
            post(client_upload).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)),
        )
        .route(
            "/support/files/{id}",
            get(client_file).delete(client_discard),
        )
        .route("/support/files/{id}/link", post(client_link))
}

pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/support/files",
            post(admin_upload).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)),
        )
        .route("/support/files/{id}", get(admin_file).delete(admin_discard))
}

pub fn public_routes() -> Router<AppState> {
    Router::new().route(&format!("{PUBLIC_PATH}/{{token}}"), get(public_file))
}

async fn client_upload(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<UploadQuery>,
    body: Bytes,
) -> ApiResult<Json<TicketFile>> {
    client.require_full()?;
    Ok(Json(
        state
            .services
            .support()
            .upload(client.actor(), &upload_name(&query), body.to_vec())
            .await?,
    ))
}

async fn client_file(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let file = state
        .services
        .support()
        .file(TicketFileId(id), client.actor())
        .await?;
    Ok(file_response(file))
}

async fn client_discard(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    state
        .services
        .support()
        .discard_file(TicketFileId(id), client.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn client_link(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let link = state
        .services
        .support()
        .file_link(TicketFileId(id), client.actor())
        .await?;
    Ok(Json(serde_json::json!({
        "url": format!("{PUBLIC_PATH}/{}", link.token),
        "name": link.name,
        "until": link.until,
    })))
}

async fn admin_upload(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<UploadQuery>,
    body: Bytes,
) -> ApiResult<Json<TicketFile>> {
    admin.require(Permission::Support)?;
    Ok(Json(
        state
            .services
            .support()
            .upload(admin.actor(), &upload_name(&query), body.to_vec())
            .await?,
    ))
}

async fn admin_file(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    admin.require(Permission::Support)?;
    let file = state
        .services
        .support()
        .file(TicketFileId(id), admin.actor())
        .await?;
    Ok(file_response(file))
}

async fn admin_discard(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Support)?;
    state
        .services
        .support()
        .discard_file(TicketFileId(id), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn public_file(State(state): State<AppState>, Path(token): Path<String>) -> Response {
    match state.services.support().file_by_link(&token).await {
        Ok(mut file) => {
            file.kind = FileKind::File;
            file_response(file)
        }
        Err(err) => ApiError::from(err).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn имя_файла_в_заголовке_экранируется() {
        assert_eq!(
            disposition(FileKind::File, "отчёт \"1\".pdf"),
            "attachment; filename=\"_____ _1_.pdf\"; filename*=UTF-8''%D0%BE%D1%82%D1%87%D1%91%D1%82%20%221%22.pdf"
        );
        assert_eq!(
            disposition(FileKind::Image, "photo.png"),
            "inline; filename=\"photo.png\"; filename*=UTF-8''photo.png"
        );
    }
}
