use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, CurrentAdmin, Meta};
use crate::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_services::ServiceError;
use sn_services::backups::{
    BUSY, BackupsView, ConfigInput, Job, Preview, S3Input, Source, TelegramTarget, UPLOAD_MAX,
    UploadView,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/backups", get(view))
        .route("/backups/settings", put(settings))
        .route("/backups/password", put(password))
        .route("/backups/telegram", put(telegram))
        .route("/backups/telegram/check", post(check_telegram))
        .route("/backups/s3", put(s3))
        .route("/backups/s3/check", post(check_s3))
        .route("/backups/run", post(run))
        .route("/backups/files/{name}", get(download).delete(remove))
        .route("/backups/files/{name}/ticket", post(ticket))
        .route("/backups/files/{name}/send", post(resend))
        .route(
            "/backups/upload",
            post(upload).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/backups/upload/{id}",
            post(append)
                .delete(discard)
                .layer(DefaultBodyLimit::disable()),
        )
        .route("/backups/preview", post(preview))
        .route("/backups/restore", post(restore))
        .route("/backups/restore/status", get(status))
}

fn allowed(admin: &CurrentAdmin) -> ApiResult<()> {
    admin.require_session()?;
    admin.require(Permission::Backups)
}

fn mapped(err: ServiceError) -> ApiError {
    let code = err.code();
    let mut api = ApiError::from(err);
    if code == BUSY {
        api.status = StatusCode::CONFLICT;
    }
    if code == sn_services::auth::TOTP_REQUIRED || code == sn_services::auth::TOTP_INVALID {
        api.status = StatusCode::UNPROCESSABLE_ENTITY;
    }
    api
}

#[derive(Debug, Deserialize)]
struct Confirm {
    password: String,
    #[serde(default)]
    code: Option<String>,
}

impl Confirm {
    fn pair(&self) -> (&str, Option<&str>) {
        (&self.password, self.code.as_deref())
    }
}

async fn view(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<BackupsView>> {
    allowed(&admin)?;
    Ok(Json(
        state
            .services
            .backups()
            .view(Some(admin.0.admin_id))
            .await
            .map_err(mapped)?,
    ))
}

async fn settings(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ConfigInput>,
) -> ApiResult<Json<BackupsView>> {
    allowed(&admin)?;
    Ok(Json(
        state
            .services
            .backups()
            .update_config(admin.0.admin_id, body)
            .await
            .map_err(mapped)?,
    ))
}

#[derive(Debug, Deserialize)]
struct PasswordBody {
    #[serde(default)]
    password: Option<String>,
    confirm: Confirm,
}

async fn password(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<PasswordBody>,
) -> ApiResult<Json<BackupsView>> {
    allowed(&admin)?;
    Ok(Json(
        state
            .services
            .backups()
            .set_password(
                admin.0.admin_id,
                body.password.as_deref(),
                body.confirm.pair(),
                &meta,
            )
            .await
            .map_err(mapped)?,
    ))
}

#[derive(Debug, Deserialize)]
struct TelegramBody {
    telegram: TelegramTarget,
    #[serde(default)]
    confirm: Option<Confirm>,
}

async fn telegram(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<TelegramBody>,
) -> ApiResult<Json<BackupsView>> {
    allowed(&admin)?;
    let confirm = body
        .confirm
        .as_ref()
        .map(Confirm::pair)
        .unwrap_or(("", None));
    Ok(Json(
        state
            .services
            .backups()
            .set_telegram(admin.0.admin_id, body.telegram, confirm, &meta)
            .await
            .map_err(mapped)?,
    ))
}

async fn check_telegram(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    allowed(&admin)?;
    let message = state
        .services
        .backups()
        .check_telegram()
        .await
        .map_err(mapped)?;
    Ok(Json(serde_json::json!({ "message": message })))
}

#[derive(Debug, Deserialize)]
struct S3Body {
    #[serde(default)]
    s3: Option<S3Input>,
    #[serde(default)]
    confirm: Option<Confirm>,
}

async fn s3(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<S3Body>,
) -> ApiResult<Json<BackupsView>> {
    allowed(&admin)?;
    let confirm = body
        .confirm
        .as_ref()
        .map(Confirm::pair)
        .unwrap_or(("", None));
    Ok(Json(
        state
            .services
            .backups()
            .set_s3(admin.0.admin_id, body.s3, confirm, &meta)
            .await
            .map_err(mapped)?,
    ))
}

#[derive(Debug, Deserialize)]
struct CheckS3Body {
    #[serde(default)]
    s3: Option<S3Input>,
}

async fn check_s3(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CheckS3Body>,
) -> ApiResult<Json<serde_json::Value>> {
    allowed(&admin)?;
    let message = state
        .services
        .backups()
        .check_s3(body.s3)
        .await
        .map_err(mapped)?;
    Ok(Json(serde_json::json!({ "message": message })))
}

async fn run(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Response> {
    allowed(&admin)?;
    let id = state
        .services
        .backups()
        .start_manual(admin.0.admin_id)
        .await
        .map_err(mapped)?;
    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({ "id": id }))).into_response())
}

async fn ticket(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(name): Path<String>,
    ApiJson(body): ApiJson<Confirm>,
) -> ApiResult<Json<serde_json::Value>> {
    allowed(&admin)?;
    let ticket = state
        .services
        .backups()
        .download_ticket(&name, admin.0.admin_id, body.pair())
        .await
        .map_err(mapped)?;
    Ok(Json(serde_json::json!({
        "url": format!(
            "/api/admin/backups/files/{}?ticket={ticket}",
            urlencoding(&name)
        ),
    })))
}

fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[derive(Debug, Default, Deserialize)]
struct TicketQuery {
    #[serde(default)]
    ticket: String,
}

async fn download(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Meta(meta): Meta,
    Path(name): Path<String>,
    ApiQuery(query): ApiQuery<TicketQuery>,
) -> ApiResult<Response> {
    allowed(&admin)?;
    let (path, size) = state
        .services
        .backups()
        .download(&name, admin.0.admin_id, &query.ticket, &meta)
        .await
        .map_err(mapped)?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| ApiError::not_found())?;
    let stream = futures_util::stream::unfold(Some(file), |file| async move {
        let mut file = file?;
        let mut buf = vec![0u8; 256 * 1024];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<Bytes, std::io::Error>(Bytes::from(buf)), Some(file)))
            }
            Err(err) => Some((Err(err), None)),
        }
    });
    let mut response = Body::from_stream(stream).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(value) = HeaderValue::from_str(&size.to_string()) {
        headers.insert(header::CONTENT_LENGTH, value);
    }
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

async fn remove(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Meta(meta): Meta,
    Path(name): Path<String>,
) -> ApiResult<Json<BackupsView>> {
    allowed(&admin)?;
    Ok(Json(
        state
            .services
            .backups()
            .delete(&name, admin.0.admin_id, &meta)
            .await
            .map_err(mapped)?,
    ))
}

async fn resend(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<BackupsView>> {
    allowed(&admin)?;
    Ok(Json(
        state
            .services
            .backups()
            .resend(&name, admin.0.admin_id)
            .await
            .map_err(mapped)?,
    ))
}

#[derive(Debug, Deserialize)]
struct UploadQuery {
    #[serde(default)]
    name: Option<String>,
}

fn too_big() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "backups.too_large",
        "Файл больше 16 ГБ",
    )
}

async fn save_body(path: &std::path::Path, body: Body, append: bool) -> ApiResult<u64> {
    if let Some(dir) = path.parent() {
        sn_services::private_files::private_dir(dir).map_err(|err| {
            ApiError::from(ServiceError::Internal(format!(
                "папка для загрузки не создаётся: {err}"
            )))
        })?;
    }
    let mut options = tokio::fs::OpenOptions::new();
    if append {
        options.append(true);
    } else {
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
    }
    let mut file = options.open(path).await.map_err(|_| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "backups.upload_gone",
            "Загрузка не найдена: начните заново",
        )
    })?;
    let mut size = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let mut stream = body.into_data_stream();
    let failed = loop {
        match stream.next().await {
            None => break None,
            Some(Ok(chunk)) => {
                size += chunk.len() as u64;
                if size > UPLOAD_MAX {
                    break Some(too_big());
                }
                if let Err(err) = file.write_all(&chunk).await {
                    break Some(ApiError::from(ServiceError::Internal(format!(
                        "на сервере не хватает места для файла: {err}"
                    ))));
                }
            }
            Some(Err(_)) => {
                break Some(ApiError::bad_request(
                    "backups.interrupted",
                    "Загрузка оборвалась. Попробуйте ещё раз",
                ));
            }
        }
    };
    let flushed = file.flush().await;
    drop(file);
    if let Some(err) = failed {
        let _ = tokio::fs::remove_file(path).await;
        return Err(err);
    }
    if flushed.is_err() {
        let _ = tokio::fs::remove_file(path).await;
        return Err(ApiError::from(ServiceError::Internal(
            "файл не сохранился".into(),
        )));
    }
    Ok(size)
}

fn upload_name(query: &UploadQuery) -> String {
    query
        .name
        .as_deref()
        .map(|n| n.chars().filter(|c| !c.is_control()).take(120).collect())
        .unwrap_or_default()
}

async fn upload(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<UploadQuery>,
    headers: axum::http::HeaderMap,
    body: Body,
) -> ApiResult<Json<UploadView>> {
    allowed(&admin)?;
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|len| len > UPLOAD_MAX) {
        return Err(too_big());
    }
    let backups = state.services.backups();
    backups.cleanup_uploads().await;
    let id = Uuid::new_v4();
    let path = backups.upload_path(id);
    let size = save_body(&path, body, false).await?;
    if size == 0 {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(ApiError::bad_request("backups.empty", "Файл пустой"));
    }
    Ok(Json(
        backups
            .upload_view(id, &upload_name(&query))
            .await
            .map_err(mapped)?,
    ))
}

async fn append(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    ApiQuery(query): ApiQuery<UploadQuery>,
    body: Body,
) -> ApiResult<Json<UploadView>> {
    allowed(&admin)?;
    let backups = state.services.backups();
    let path = backups.upload_path(id);
    save_body(&path, body, true).await?;
    Ok(Json(
        backups
            .upload_view(id, &upload_name(&query))
            .await
            .map_err(mapped)?,
    ))
}

async fn discard(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    allowed(&admin)?;
    state
        .services
        .backups()
        .discard_upload(id)
        .await
        .map_err(mapped)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct PreviewBody {
    source: Source,
    #[serde(default)]
    password: Option<String>,
}

async fn preview(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<PreviewBody>,
) -> ApiResult<Json<Preview>> {
    allowed(&admin)?;
    Ok(Json(
        state
            .services
            .backups()
            .preview(&body.source, body.password.as_deref())
            .await
            .map_err(mapped)?,
    ))
}

#[derive(Debug, Deserialize)]
struct RestoreBody {
    source: Source,
    #[serde(default)]
    password: Option<String>,
    confirm: Confirm,
}

async fn restore(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<RestoreBody>,
) -> ApiResult<Response> {
    allowed(&admin)?;
    let watch = state
        .services
        .backups()
        .start_restore(
            body.source,
            body.password,
            admin.0.admin_id,
            body.confirm.pair(),
            &meta,
        )
        .await
        .map_err(mapped)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "watch": watch })),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
struct StatusQuery {
    watch: String,
}

async fn status(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<StatusQuery>,
) -> ApiResult<Json<Job>> {
    state
        .services
        .backups()
        .status(&query.watch)
        .map(Json)
        .ok_or_else(ApiError::not_found)
}
