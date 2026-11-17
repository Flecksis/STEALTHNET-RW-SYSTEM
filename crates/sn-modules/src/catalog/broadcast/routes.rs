use super::{
    Audience, Broadcast, BroadcastError, ButtonSpec, Draft, IMAGE_MAX_BYTES, ImageMeta, ListItem,
    Options, Reach, TestReport,
};
use crate::catalog::web::{Admin, WebError};
use crate::context::ModuleContext;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sn_domain::access::Permission;
use sn_domain::ids::ClientId;
use sn_domain::paging::{Page, PageRequest};

const UPLOAD_LIMIT: usize = IMAGE_MAX_BYTES + 64 * 1024;

impl From<BroadcastError> for WebError {
    fn from(err: BroadcastError) -> Self {
        match err {
            BroadcastError::Rejected { code, message } => WebError::rejected(code, message),
            BroadcastError::Module(err) => err.into(),
        }
    }
}

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/options", get(options))
        .route("/broadcasts", get(list).post(create))
        .route("/broadcasts/{id}", get(view).put(update).delete(remove))
        .route("/broadcasts/{id}/send", post(send))
        .route("/broadcasts/{id}/cancel", post(cancel))
        .route("/broadcasts/{id}/copy", post(copy))
        .route("/audience", post(audience))
        .route("/test", post(test))
        .route(
            "/images",
            post(upload).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)),
        )
        .route("/images/{id}", get(image))
        .with_state(ctx)
}

fn parse<T: DeserializeOwned>(body: &Bytes) -> Result<T, WebError> {
    serde_json::from_slice(body).map_err(|err| {
        tracing::debug!(error = %err, "рассылки: тело запроса не разобрано");
        WebError::rejected(
            "validation.invalid_value",
            "Запрос не разобран: обновите страницу и попробуйте ещё раз",
        )
    })
}

async fn options(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<Options>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::options(&ctx).await?))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

async fn list(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<ListItem>>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(
        super::list(&ctx, PageRequest::new(query.limit, query.after)).await?,
    ))
}

async fn create(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    body: Bytes,
) -> Result<Json<Broadcast>, WebError> {
    admin.require(Permission::Marketing)?;
    let draft: Draft = parse(&body)?;
    Ok(Json(
        super::create(&ctx, &draft, Some(admin.0.admin_id)).await?,
    ))
}

async fn view(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<Broadcast>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::view(&ctx, id).await?))
}

async fn update(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Json<Broadcast>, WebError> {
    admin.require(Permission::Marketing)?;
    let draft: Draft = parse(&body)?;
    Ok(Json(super::update(&ctx, id, &draft).await?))
}

async fn remove(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, WebError> {
    admin.require(Permission::Marketing)?;
    super::delete(&ctx, id).await?;
    Ok(Json(serde_json::json!({ "deleted": id })))
}

#[derive(Deserialize)]
struct SendBody {
    #[serde(default)]
    at: Option<DateTime<Utc>>,
}

async fn send(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Json<Broadcast>, WebError> {
    admin.require(Permission::Marketing)?;
    let body: SendBody = if body.is_empty() {
        SendBody { at: None }
    } else {
        parse(&body)?
    };
    Ok(Json(
        super::start(&ctx, id, body.at, Some(admin.0.admin_id)).await?,
    ))
}

async fn cancel(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<Broadcast>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::cancel(&ctx, id, Some(admin.0.admin_id)).await?))
}

async fn copy(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<Broadcast>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::copy(&ctx, id, Some(admin.0.admin_id)).await?))
}

#[derive(Deserialize)]
struct AudienceBody {
    audience: Audience,
}

async fn audience(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    body: Bytes,
) -> Result<Json<Reach>, WebError> {
    admin.require(Permission::Marketing)?;
    let body: AudienceBody = parse(&body)?;
    Ok(Json(super::reach(&ctx, body.audience).await?))
}

#[derive(Deserialize)]
struct TestBody {
    client_id: ClientId,
    #[serde(default)]
    text: String,
    #[serde(default)]
    image_id: Option<i64>,
    #[serde(default)]
    buttons: Vec<ButtonSpec>,
}

async fn test(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    body: Bytes,
) -> Result<Json<TestReport>, WebError> {
    admin.require(Permission::Marketing)?;
    let body: TestBody = parse(&body)?;
    let draft = Draft {
        text: body.text,
        image_id: body.image_id,
        buttons: body.buttons,
        audience: Audience::All,
    };
    Ok(Json(
        super::send_test(&ctx, &draft, body.client_id, Some(admin.0.admin_id)).await?,
    ))
}

async fn upload(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ImageMeta>, WebError> {
    admin.require(Permission::Marketing)?;
    let declared = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    if !matches!(declared.as_str(), "image/png" | "image/jpeg") {
        return Err(WebError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "broadcast.image",
            "Нужна картинка JPEG или PNG",
        ));
    }
    Ok(Json(
        super::upload_image(&ctx, &body, Some(admin.0.admin_id)).await?,
    ))
}

async fn image(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Response, WebError> {
    admin.require(Permission::Marketing)?;
    let (content_type, bytes) = super::image_file(&ctx, id).await?;
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&content_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400, immutable"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    Ok(response)
}
