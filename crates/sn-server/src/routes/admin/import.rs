use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_services::ServiceError;
use sn_services::legacy::runs::{RunView, StartOptions};
use sn_services::legacy::{ImportView, MAX_UPLOAD, Source, import_dir};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/import", get(view))
        .route(
            "/import/upload",
            post(upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/import/{id}", get(run).delete(discard))
        .route("/import/{id}/password", post(password))
        .route("/import/{id}/start", post(start))
        .route("/import/{id}/check", post(recheck))
        .route("/import/{id}/report", get(report))
}

fn allowed(admin: &CurrentAdmin) -> ApiResult<()> {
    admin.require_session()?;
    admin.require(Permission::Settings)?;
    admin.require(Permission::Money)
}

fn mapped(err: ServiceError) -> ApiError {
    let busy = err.code() == "import.busy";
    let mut api = ApiError::from(err);
    if busy {
        api.status = StatusCode::CONFLICT;
    }
    api
}

async fn view(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<ImportView>> {
    allowed(&admin)?;
    Ok(Json(state.services.legacy().view().await.map_err(mapped)?))
}

async fn run(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<RunView>> {
    allowed(&admin)?;
    Ok(Json(state.services.legacy().run(id).await.map_err(mapped)?))
}

#[derive(Debug, Deserialize)]
struct UploadQuery {
    source: String,
    #[serde(default)]
    name: Option<String>,
}

fn too_big() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "import.too_large",
        "Файл больше 2 ГБ. Нужна выгрузка базы, а не архив сервера целиком",
    )
}

async fn upload(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<UploadQuery>,
    headers: axum::http::HeaderMap,
    body: Body,
) -> ApiResult<Json<RunView>> {
    allowed(&admin)?;
    let Some(source) = Source::parse(&query.source) else {
        return Err(ApiError::bad_request(
            "import.source",
            "Выберите, откуда переносим: STEALTHNET или бот Бедолаги",
        ));
    };
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|len| len > MAX_UPLOAD) {
        return Err(too_big());
    }
    if state.services.legacy().busy().await.map_err(mapped)? {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "import.busy",
            "Перенос уже идёт. Дождитесь окончания и загрузите файл снова",
        ));
    }
    let dir = import_dir();
    tokio::fs::create_dir_all(&dir).await.map_err(|err| {
        ApiError::from(ServiceError::Internal(format!(
            "папка для выгрузки не создаётся: {err}"
        )))
    })?;
    let path = dir.join(format!("{}.upload", Uuid::new_v4().simple()));
    let mut file = tokio::fs::File::create(&path).await.map_err(|err| {
        ApiError::from(ServiceError::Internal(format!(
            "файл выгрузки не создаётся: {err}"
        )))
    })?;
    let mut stream = body.into_data_stream();
    let mut size: u64 = 0;
    let failed = loop {
        match stream.next().await {
            None => break None,
            Some(Ok(chunk)) => {
                size += chunk.len() as u64;
                if size > MAX_UPLOAD {
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
                    "import.interrupted",
                    "Загрузка оборвалась. Попробуйте ещё раз",
                ));
            }
        }
    };
    let flushed = file.flush().await;
    drop(file);
    if let Some(err) = failed {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(err);
    }
    if flushed.is_err() || size == 0 {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(ApiError::bad_request("import.empty", "Файл пустой"));
    }
    let name = query.name.unwrap_or_default();
    Ok(Json(
        state
            .services
            .legacy()
            .uploaded(source, &name, size, &path, Some(admin.0.admin_id))
            .await
            .map_err(mapped)?,
    ))
}

#[derive(Debug, Deserialize)]
struct PasswordBody {
    password: String,
}

async fn password(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    ApiJson(body): ApiJson<PasswordBody>,
) -> ApiResult<Json<RunView>> {
    allowed(&admin)?;
    let password = body.password.trim().to_string();
    if password.is_empty() || password.len() > 256 {
        return Err(ApiError::bad_request(
            "import.password",
            "Введите пароль от архива",
        ));
    }
    Ok(Json(
        state
            .services
            .legacy()
            .password(id, password)
            .await
            .map_err(mapped)?,
    ))
}

async fn start(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> ApiResult<Json<RunView>> {
    allowed(&admin)?;
    let options: StartOptions = if body.iter().all(u8::is_ascii_whitespace) {
        StartOptions::default()
    } else {
        serde_json::from_slice(&body).map_err(|_| {
            ApiError::from(ServiceError::rejected(
                "import.options",
                "Не удалось прочитать выбор для переноса",
            ))
        })?
    };
    Ok(Json(
        state
            .services
            .legacy()
            .start_with(id, Some(admin.0.admin_id), options)
            .await
            .map_err(mapped)?,
    ))
}

async fn recheck(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<RunView>> {
    allowed(&admin)?;
    Ok(Json(
        state.services.legacy().recheck(id).await.map_err(mapped)?,
    ))
}

async fn discard(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    allowed(&admin)?;
    state.services.legacy().discard(id).await.map_err(mapped)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn report(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    allowed(&admin)?;
    let (name, bytes) = state.services.legacy().report(id).await.map_err(mapped)?;
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"));
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/csv; charset=utf-8"),
            ),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    )
        .into_response())
}
