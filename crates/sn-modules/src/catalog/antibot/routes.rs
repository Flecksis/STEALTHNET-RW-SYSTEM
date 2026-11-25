use super::captcha::{self, Probe, Provider};
use super::store::{self, JournalFilter, JournalItem, MarkItem, Summary};
use super::{AntibotSettings, active, signups};
use crate::catalog::web::{Admin, ClientAddr, WebError};
use crate::context::{ModuleContext, RateLimit};
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_domain::ids::ClientId;
use sn_domain::paging::{Page, PageRequest};

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/summary", get(summary))
        .route("/journal", get(journal))
        .route("/marks", get(marks))
        .route("/marks/{client_id}", get(mark))
        .route("/marks/{client_id}/clear", post(clear))
        .route("/captcha/check", post(check_captcha))
        .with_state(ctx)
}

const KEY_CHECKS: RateLimit = RateLimit {
    scope: "antibot_key_checks",
    limit: 20,
    window_seconds: 600,
};

#[derive(Deserialize)]
struct KeyCheck {
    provider: Provider,
    #[serde(default)]
    site_key: String,
    #[serde(default)]
    secret: Option<String>,
}

#[derive(Serialize)]
struct KeyCheckView {
    ok: bool,
    text: String,
}

fn bad_key(text: impl Into<String>) -> WebError {
    WebError::rejected("antibot.captcha_key", text)
}

async fn check_captcha(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Json(body): Json<KeyCheck>,
) -> Result<Json<KeyCheckView>, WebError> {
    admin.require(Permission::Modules)?;
    if body.provider == Provider::None {
        return Err(bad_key("Выберите капчу"));
    }
    let site_key = captcha::clean_key(&body.site_key).ok_or_else(|| {
        bad_key("Укажите ключ сайта: латинские буквы, цифры, точка, дефис и подчёркивание")
    })?;
    let typed = body
        .secret
        .as_deref()
        .map(str::trim)
        .filter(|secret| !secret.is_empty());
    let secret = match typed {
        Some(raw) => captcha::clean_key(raw),
        None => {
            let config = ctx.core.module_config(sn_domain::module::ModuleId::Antibot);
            let saved = AntibotSettings::lenient(&config).captcha == body.provider;
            config
                .text("captcha_secret")
                .filter(|_| saved)
                .and_then(captcha::clean_key)
        }
    }
    .ok_or_else(|| {
        bad_key("Укажите секретный ключ: латинские буквы, цифры, точка, дефис и подчёркивание")
    })?;
    captcha::check_keys(body.provider, &site_key, &secret).map_err(bad_key)?;
    if let Some(made) = captcha::foreign(body.provider, &secret) {
        return Err(bad_key(captcha::foreign_text(body.provider, made)));
    }
    if !ctx
        .core
        .within_rate_limit(KEY_CHECKS, &admin.0.admin_id.0.to_string())
        .await?
    {
        return Err(WebError::new(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "antibot.captcha_checks",
            "Проверок ключей — не больше 20 за 10 минут. Попробуйте позже",
        ));
    }
    let outcome = captcha::probe(&captcha::Service::of(&ctx), body.provider, &secret).await;
    if outcome == Probe::Accepted {
        let mut conn = ctx.store.conn().await?;
        captcha::recovered(&mut conn).await?;
    }
    Ok(Json(match outcome {
        Probe::Accepted => KeyCheckView {
            ok: true,
            text: "Секретный ключ подошёл. Ключ сайта и список доменов проверяет браузер: откройте вход на сайт и пройдите капчу".into(),
        },
        Probe::Refused => KeyCheckView {
            ok: false,
            text: format!(
                "{} не принимает этот секретный ключ. Скопируйте его ещё раз",
                body.provider.title()
            ),
        },
        Probe::Test => KeyCheckView {
            ok: false,
            text: "Это проверочный ключ Cloudflare: он отвечает всем одинаково и никого не проверяет. Вставьте настоящий".into(),
        },
        Probe::Unavailable(problem) => KeyCheckView {
            ok: false,
            text: format!("Проверить не получилось: {problem}. Сервис капчи должен открываться с сервера панели"),
        },
    }))
}

#[derive(Serialize)]
struct CaptchaState {
    secret_set: bool,
    ready: bool,
    problem: Option<captcha::Health>,
}

#[derive(Serialize)]
struct SummaryView {
    settings: AntibotSettings,
    captcha: CaptchaState,
    site: SiteState,
    #[serde(flatten)]
    counts: Summary,
}

#[derive(Serialize)]
struct SiteState {
    day: signups::Day,
    hidden: Option<signups::HiddenAddress>,
    trusted: Option<String>,
}

async fn site_state(
    db: &mut sn_store::Conn,
    address: ClientAddr,
) -> Result<SiteState, sn_store::StoreError> {
    let day = signups::day(db).await?;
    let hidden = match signups::hidden(db).await? {
        Some(found) => Some(found),
        None => match signups::Origin::of(address.0) {
            signups::Origin::Hidden {
                kind: signups::Hidden::Cloudflare,
                address,
            } => Some(signups::HiddenAddress {
                kind: signups::Hidden::Cloudflare,
                address: address.to_string(),
                at: chrono::Utc::now(),
            }),
            _ => None,
        },
    };
    let trusted = hidden
        .as_ref()
        .filter(|found| found.kind == signups::Hidden::Cloudflare)
        .map(|_| signups::trusted_line());
    Ok(SiteState {
        day,
        hidden,
        trusted,
    })
}

fn captcha_state(
    ctx: &ModuleContext,
    settings: &AntibotSettings,
    health: Option<captcha::Health>,
) -> CaptchaState {
    let config = ctx.core.module_config(sn_domain::module::ModuleId::Antibot);
    let ready = captcha::Captcha::of(&config).is_some();
    CaptchaState {
        secret_set: config.text("captcha_secret").is_some(),
        ready,
        problem: health.filter(|health| ready && health.provider == settings.captcha),
    }
}

async fn summary(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    address: ClientAddr,
) -> Result<Json<SummaryView>, WebError> {
    admin.require(Permission::ClientsRead)?;
    let mut conn = ctx.store.conn().await?;
    let settings = active(&mut conn).await?.unwrap_or_default();
    let counts = store::summary(&mut conn).await?;
    let health = captcha::health(&mut conn).await?;
    let state = captcha_state(&ctx, &settings, health);
    let site = site_state(&mut conn, address).await?;
    Ok(Json(SummaryView {
        settings,
        captcha: state,
        site,
        counts,
    }))
}

#[derive(Deserialize)]
struct JournalQuery {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    client: Option<i64>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

async fn journal(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<JournalQuery>,
) -> Result<Json<Page<JournalItem>>, WebError> {
    admin.require(Permission::ClientsRead)?;
    let filter = JournalFilter::parse(query.kind.as_deref()).ok_or_else(|| {
        WebError::rejected(
            "validation.invalid_value",
            "kind: all, signup, trial, promo, referral, roulette, contest или cleared",
        )
    })?;
    let mut conn = ctx.store.conn().await?;
    Ok(Json(
        store::journal(
            &mut conn,
            filter,
            query.client.map(ClientId),
            PageRequest::new(query.limit, query.after),
        )
        .await?,
    ))
}

#[derive(Deserialize)]
struct PageQuery {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

async fn marks(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<MarkItem>>, WebError> {
    admin.require(Permission::ClientsRead)?;
    let mut conn = ctx.store.conn().await?;
    Ok(Json(
        store::open_marks(&mut conn, PageRequest::new(query.limit, query.after)).await?,
    ))
}

#[derive(Serialize)]
struct MarkView {
    marked: bool,
    reasons: Vec<store::Reason>,
    marked_at: Option<chrono::DateTime<chrono::Utc>>,
    confirmed_at: Option<chrono::DateTime<chrono::Utc>>,
    cleared_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn mark(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(client_id): Path<i64>,
) -> Result<Json<MarkView>, WebError> {
    admin.require(Permission::ClientsRead)?;
    let mut conn = ctx.store.conn().await?;
    let found = store::mark_of(&mut conn, ClientId(client_id)).await?;
    Ok(Json(match found {
        Some(mark) => MarkView {
            marked: mark.cleared_at.is_none(),
            reasons: store::reasons(&mark.reasons),
            marked_at: Some(mark.marked_at),
            confirmed_at: mark.confirmed_at,
            cleared_at: mark.cleared_at,
        },
        None => MarkView {
            marked: false,
            reasons: Vec::new(),
            marked_at: None,
            confirmed_at: None,
            cleared_at: None,
        },
    }))
}

async fn clear(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(client_id): Path<i64>,
) -> Result<Json<serde_json::Value>, WebError> {
    admin.require(Permission::ClientsWrite)?;
    let cleared = super::clear(&ctx, ClientId(client_id), Some(admin.0.admin_id)).await?;
    Ok(Json(serde_json::json!({ "cleared": cleared })))
}
