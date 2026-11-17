use crate::cookies::{self, SameSite};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, CLIENT_COOKIE, CurrentClient, Meta, request_credential};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::ClientChannel;
use sn_domain::ids::{PeriodId, SubscriptionId, TicketId};
use sn_domain::money::{Money, parse_units};
use sn_domain::support::{Ticket, TicketChannel};
use sn_services::checkout::{CartRequest, Channel, CheckoutRequest, CheckoutResult};
use sn_services::login::TelegramLoginStatus;
use sn_services::support::{OpenTicket, ReplyRequest, TicketThread};
use sn_services::two_factor::{CabinetLogin, TwoFactor};
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/miniapp", post(miniapp_login))
        .route("/auth/telegram", post(telegram_start))
        .route("/auth/telegram/status", post(telegram_status))
        .route("/auth/email", post(email_start))
        .route("/auth/email/verify", post(email_verify))
        .route("/auth/totp", post(totp_login))
        .route("/auth/logout", post(logout))
        .route("/me", get(me))
        .route("/security", get(security))
        .route("/security/totp/setup", post(totp_setup))
        .route("/security/totp/enable", post(totp_enable))
        .route("/security/totp/disable", post(totp_disable))
        .route("/catalog", get(catalog))
        .route("/quote", post(quote))
        .route("/checkout", post(checkout))
        .route("/topup", post(topup))
        .route("/trial", post(trial))
        .route("/payments/{id}", get(payment))
        .route("/subscriptions", get(subscriptions))
        .route("/subscriptions/{id}", get(subscription))
        .route("/subscriptions/{id}/autorenew", put(autorenew))
        .route("/subscriptions/{id}/devices", get(devices))
        .route(
            "/subscriptions/{id}/devices/{hwid}",
            delete(disconnect_device),
        )
        .route("/subscriptions/{id}/reissue", post(reissue))
        .route("/support/tickets", get(tickets).post(open_ticket))
        .route("/support/tickets/{id}", get(ticket).post(reply))
        .route("/support/tickets/{id}/close", post(close_ticket))
}

fn channel(client: &CurrentClient) -> Channel {
    match client.channel() {
        ClientChannel::Miniapp => Channel::Miniapp,
        ClientChannel::Bot => Channel::Bot,
        ClientChannel::Cabinet => Channel::Cabinet,
    }
}

fn session_response(
    state: &AppState,
    login: &sn_services::auth::ClientLogin,
    body: impl serde::Serialize,
) -> Response {
    let cookie = cookies::session(
        state,
        CLIENT_COOKIE,
        &login.token,
        Some(login.expires_at),
        SameSite::Lax,
    );
    cookies::with_cookie(cookie, Json(body))
}

#[derive(Deserialize)]
struct MiniappLogin {
    init_data: String,
}

async fn miniapp_login(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<MiniappLogin>,
) -> ApiResult<Json<sn_services::auth::ClientLogin>> {
    Ok(Json(
        state
            .services
            .auth()
            .miniapp_login(&body.init_data, &meta)
            .await?,
    ))
}

#[derive(Debug, Default, Deserialize)]
struct TelegramStart {
    #[serde(default, rename = "ref")]
    referral: Option<String>,
}

async fn telegram_start(
    State(state): State<AppState>,
    Meta(meta): Meta,
    body: Option<ApiJson<TelegramStart>>,
) -> ApiResult<Json<sn_services::login::TelegramLoginStart>> {
    let referral = body.and_then(|b| b.0.referral);
    Ok(Json(
        state
            .services
            .auth()
            .telegram_login_start_with(&meta, referral.as_deref())
            .await?,
    ))
}

#[derive(Deserialize)]
struct TelegramPoll {
    poll_token: String,
}

async fn telegram_status(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<TelegramPoll>,
) -> ApiResult<Response> {
    let status = state
        .services
        .auth()
        .telegram_login_status(&body.poll_token, &meta)
        .await?;
    Ok(match &status {
        TelegramLoginStatus::Confirmed { login } => session_response(&state, login, &status),
        _ => cookies::with_cookie(None, Json(&status)),
    })
}

#[derive(Deserialize)]
struct EmailStart {
    email: String,
}

async fn email_start(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<EmailStart>,
) -> ApiResult<Json<sn_services::login::EmailLoginStart>> {
    Ok(Json(
        state
            .services
            .auth()
            .email_login_start(&body.email, &meta)
            .await?,
    ))
}

#[derive(Deserialize)]
struct EmailVerify {
    email: String,
    code: String,
}

async fn email_verify(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<EmailVerify>,
) -> ApiResult<Response> {
    let login = state
        .services
        .auth()
        .email_login_verify(&body.email, &body.code, &meta)
        .await?;
    Ok(match &login {
        CabinetLogin::Session(session) => session_response(&state, session, &login),
        CabinetLogin::TotpRequired(_) => cookies::with_cookie(None, Json(&login)),
    })
}

#[derive(Deserialize)]
struct TotpLogin {
    challenge: String,
    code: String,
}

async fn totp_login(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<TotpLogin>,
) -> ApiResult<Response> {
    let login = state
        .services
        .auth()
        .cabinet_totp_login(&body.challenge, &body.code, &meta)
        .await?;
    Ok(session_response(&state, &login, &login))
}

async fn security(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<TwoFactor>> {
    client.require_full()?;
    Ok(Json(
        state
            .services
            .auth()
            .client_two_factor(client.0.client_id)
            .await?,
    ))
}

async fn totp_setup(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<sn_services::auth::TotpSetup>> {
    client.require_full()?;
    Ok(Json(
        state
            .services
            .auth()
            .client_totp_setup(client.0.client_id)
            .await?,
    ))
}

#[derive(Deserialize)]
struct TotpEnable {
    secret: String,
    code: String,
}

async fn totp_enable(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TotpEnable>,
) -> ApiResult<Json<TwoFactor>> {
    client.require_full()?;
    Ok(Json(
        state
            .services
            .auth()
            .client_totp_enable(client.0.client_id, &body.secret, &body.code)
            .await?,
    ))
}

#[derive(Deserialize)]
struct TotpDisable {
    code: String,
}

async fn totp_disable(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TotpDisable>,
) -> ApiResult<Json<TwoFactor>> {
    client.require_full()?;
    Ok(Json(
        state
            .services
            .auth()
            .client_totp_disable(client.0.client_id, &body.code)
            .await?,
    ))
}

async fn logout(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
) -> ApiResult<Response> {
    if let Some(credential) = request_credential(&state, &method, &headers, CLIENT_COOKIE)? {
        state
            .services
            .auth()
            .client_logout(credential.token())
            .await?;
    }
    let cookie = cookies::session(&state, CLIENT_COOKIE, "", None, SameSite::Lax);
    Ok(cookies::with_cookie(
        cookie,
        Json(serde_json::json!({ "ok": true })),
    ))
}

async fn me(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    let card = state.services.clients().card(client.0.client_id).await?;
    Ok(Json(serde_json::json!({
        "client": card.client,
        "balance": card.balance,
        "referral_balance": card.referral_balance,
        "currency": state.services.settings().currency,
        "channel": client.0.channel,
        "full_access": client.0.full_access,
        "two_factor": card.two_factor,
    })))
}

async fn catalog(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    let offers = state
        .services
        .checkout()
        .catalog_in(Some(client.0.channel))
        .await?;
    Ok(Json(serde_json::json!({
        "currency": state.services.settings().currency,
        "tariffs": offers,
    })))
}

async fn quote(
    client: CurrentClient,
    Meta(meta): Meta,
    State(state): State<AppState>,
    ApiJson(cart): ApiJson<CartRequest>,
) -> ApiResult<Json<sn_services::checkout::QuoteView>> {
    let address = meta.ip.as_deref().and_then(|ip| ip.parse().ok());
    Ok(Json(
        state
            .services
            .checkout()
            .quote_from(client.0.client_id, &cart, address)
            .await?,
    ))
}

#[derive(Deserialize)]
struct CheckoutBody {
    cart: CartRequest,
    provider: String,
    #[serde(default)]
    return_url: Option<String>,
}

async fn checkout(
    client: CurrentClient,
    Meta(meta): Meta,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CheckoutBody>,
) -> ApiResult<Json<CheckoutResult>> {
    client.require_full()?;
    let request = CheckoutRequest {
        cart: body.cart,
        provider: body.provider,
        channel: channel(&client),
        return_url: body.return_url,
    };
    let address = meta.ip.as_deref().and_then(|ip| ip.parse().ok());
    Ok(Json(
        state
            .services
            .checkout()
            .start_from(client.0.client_id, &request, address)
            .await?,
    ))
}

#[derive(Deserialize)]
struct TopUpBody {
    amount: String,
    provider: String,
}

async fn topup(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TopUpBody>,
) -> ApiResult<Json<CheckoutResult>> {
    client.require_full()?;
    let currency = state.services.settings().currency;
    let minor = parse_units(body.amount.trim(), currency.into()).ok_or_else(|| {
        ApiError::bad_request(
            "validation.amount",
            "сумма пополнения указана неверно, например 10.00",
        )
    })?;
    let limits = state.services.settings().topup_limits();
    let amount = Money::new(minor, currency);
    if amount.minor < limits.min.minor || amount.minor > limits.max.minor {
        return Err(ApiError::bad_request(
            "balance.topup_out_of_range",
            format!(
                "сумма пополнения должна быть от {} до {}",
                limits.min, limits.max
            ),
        ));
    }
    Ok(Json(
        state
            .services
            .checkout()
            .topup(
                client.0.client_id,
                amount,
                body.provider.trim(),
                channel(&client),
            )
            .await?,
    ))
}

async fn trial(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    let order_id = state
        .services
        .checkout()
        .start_trial(client.0.client_id)
        .await?;
    Ok(Json(serde_json::json!({ "order_id": order_id })))
}

async fn payment(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<sn_services::clients::PaymentStatusView>> {
    Ok(Json(
        state
            .services
            .clients()
            .payment_status(client.0.client_id, id)
            .await?,
    ))
}

async fn subscriptions(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<sn_services::subscriptions::SubscriptionView>>> {
    Ok(Json(
        state
            .services
            .subscriptions()
            .list_for_client(client.0.client_id)
            .await?,
    ))
}

async fn subscription(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<sn_services::subscriptions::SubscriptionView>> {
    Ok(Json(
        state
            .services
            .subscriptions()
            .view(SubscriptionId(id), client.actor(), true)
            .await?,
    ))
}

#[derive(Deserialize)]
struct AutorenewBody {
    enabled: bool,
    #[serde(default)]
    period_id: Option<PeriodId>,
}

async fn autorenew(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<AutorenewBody>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    state
        .services
        .subscriptions()
        .set_autorenew(
            SubscriptionId(id),
            body.enabled,
            body.period_id,
            client.actor(),
        )
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn devices(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<sn_services::subscriptions::DeviceView>>> {
    Ok(Json(
        state
            .services
            .subscriptions()
            .devices(SubscriptionId(id), client.actor())
            .await?,
    ))
}

async fn disconnect_device(
    client: CurrentClient,
    State(state): State<AppState>,
    Path((id, hwid)): Path<(i64, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    state
        .services
        .subscriptions()
        .disconnect_device(SubscriptionId(id), &hwid, client.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn reissue(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    let url = state
        .services
        .subscriptions()
        .reissue_link(SubscriptionId(id), client.actor())
        .await?;
    Ok(Json(serde_json::json!({ "subscription_url": url })))
}

async fn tickets(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Ticket>>> {
    Ok(Json(
        state
            .services
            .support()
            .for_client(client.0.client_id)
            .await?,
    ))
}

#[derive(Deserialize)]
struct OpenTicketBody {
    #[serde(default)]
    subject: Option<String>,
    body: String,
    #[serde(default)]
    subscription_id: Option<SubscriptionId>,
}

async fn open_ticket(
    client: CurrentClient,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<OpenTicketBody>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    let channel = match client.channel() {
        ClientChannel::Cabinet => TicketChannel::Cabinet,
        ClientChannel::Miniapp => TicketChannel::Miniapp,
        ClientChannel::Bot => TicketChannel::Bot,
    };
    let id = state
        .services
        .support()
        .open(
            client.0.client_id,
            &OpenTicket {
                subscription_id: body.subscription_id,
                channel,
                subject: body.subject,
                body: body.body,
            },
        )
        .await?;
    Ok(Json(serde_json::json!({ "ticket_id": id })))
}

async fn ticket(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<TicketThread>> {
    Ok(Json(
        state
            .services
            .support()
            .thread(TicketId(id), client.actor())
            .await?,
    ))
}

#[derive(Deserialize)]
struct ReplyBody {
    body: String,
}

async fn reply(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<ReplyBody>,
) -> ApiResult<Json<TicketThread>> {
    client.require_full()?;
    let support = state.services.support();
    support
        .reply(
            TicketId(id),
            &ReplyRequest {
                body: body.body,
                close: false,
            },
            client.actor(),
        )
        .await?;
    Ok(Json(support.thread(TicketId(id), client.actor()).await?))
}

async fn close_ticket(
    client: CurrentClient,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    client.require_full()?;
    state
        .services
        .support()
        .close(TicketId(id), client.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use crate::{AppState, ServerConfig, router};
    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::{HeaderMap, Method, Request, StatusCode, header};
    use serde_json::{Value, json};
    use sn_domain::client::{RegistrationSource, TelegramProfile};
    use sn_domain::ids::{ClientId, TicketId};
    use sn_domain::settings::keys;
    use sn_domain::support::{TicketChannel, TicketStatus};
    use sn_modules::ModuleRegistry;
    use sn_payments::ProviderRegistry;
    use sn_services::auth::{RequestMeta, sign_init_data};
    use sn_services::support::OpenTicket;
    use sn_services::{Actor, EnvFallbacks, ServiceDeps, Services, SystemClock};
    use sn_store::testing::{TestDb, test_store};
    use sn_store::{Db, SessionChannel};
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tower::ServiceExt;

    const BOT: &str = "123456:TEST-token";
    const PANEL: &str = "https://panel.example.com";
    const PASSWORD: &str = "очень-надёжный-пароль";

    async fn state() -> Option<(TestDb, AppState)> {
        let db = test_store().await?;
        let services = Services::new(ServiceDeps {
            store: db.store.clone(),
            payments: Arc::new(ProviderRegistry::builtin()),
            modules: Arc::new(ModuleRegistry::catalog_only()),
            secrets: None,
            clock: Arc::new(SystemClock),
            env: EnvFallbacks {
                bot_token: Some(BOT.into()),
                ..EnvFallbacks::default()
            },
        });
        services.reload().await.unwrap();
        let config = ServerConfig {
            public_url: Some(PANEL.into()),
            ..ServerConfig::default()
        };
        Some((db, AppState::new(config, services)))
    }

    async fn send(
        state: &AppState,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
        send_from(state, None, method, path, headers, body).await
    }

    async fn send_from(
        state: &AppState,
        peer: Option<SocketAddr>,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut builder = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let mut request = match body {
            Some(body) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        if let Some(peer) = peer {
            request.extensions_mut().insert(ConnectInfo(peer));
        }
        let response = router(state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            headers,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn percent(value: &str) -> String {
        value
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() {
                    char::from(b).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    }

    fn init_data(state: &AppState) -> String {
        let user = r#"{"id":5093118422,"first_name":"Алексей","username":"morozov_a"}"#;
        let auth_date = state.services.now().timestamp().to_string();
        let hash = sign_init_data(&[("auth_date", &auth_date), ("user", user)], BOT);
        format!("auth_date={auth_date}&user={}&hash={hash}", percent(user))
    }

    async fn cabinet_client(state: &AppState) -> (ClientId, String) {
        let (client, _) = state
            .services
            .clients()
            .ensure_telegram(
                &TelegramProfile {
                    id: 5_093_118_422,
                    username: Some("morozov_a".into()),
                    first_name: Some("Алексей".into()),
                    last_name: None,
                    language_code: Some("ru".into()),
                    is_premium: false,
                },
                RegistrationSource::Bot,
                None,
            )
            .await
            .unwrap();
        let login = state
            .services
            .auth()
            .client_session_for(
                client.id,
                SessionChannel::Cabinet,
                &RequestMeta::default(),
                false,
            )
            .await
            .unwrap();
        (client.id, login.token)
    }

    async fn open_ticket(state: &AppState, client: ClientId) -> TicketId {
        state
            .services
            .support()
            .open(
                client,
                &OpenTicket {
                    subscription_id: None,
                    channel: TicketChannel::Cabinet,
                    subject: Some("Чек".into()),
                    body: "Не пришёл чек об оплате".into(),
                },
            )
            .await
            .unwrap()
    }

    async fn ticket_status(state: &AppState, client: ClientId, ticket: TicketId) -> TicketStatus {
        state
            .services
            .support()
            .thread(ticket, Actor::Client(client))
            .await
            .unwrap()
            .ticket
            .status
    }

    async fn promo_state() -> Option<(TestDb, AppState)> {
        let db = test_store().await?;
        let services = Services::new(ServiceDeps {
            store: db.store.clone(),
            payments: Arc::new(ProviderRegistry::builtin()),
            modules: Arc::new(ModuleRegistry::new(sn_modules::catalog::all()).unwrap()),
            secrets: None,
            clock: Arc::new(SystemClock),
            env: EnvFallbacks {
                bot_token: Some(BOT.into()),
                ..EnvFallbacks::default()
            },
        });
        services.reload().await.unwrap();
        let config = ServerConfig {
            public_url: Some(PANEL.into()),
            ..ServerConfig::default()
        };
        Some((db, AppState::new(config, services)))
    }

    async fn session(state: &AppState, telegram_id: i64) -> String {
        let (client, _) = state
            .services
            .clients()
            .ensure_telegram(
                &TelegramProfile {
                    id: telegram_id,
                    username: None,
                    first_name: Some("Клиент".into()),
                    last_name: None,
                    language_code: Some("ru".into()),
                    is_premium: false,
                },
                RegistrationSource::Bot,
                None,
            )
            .await
            .unwrap();
        state
            .services
            .auth()
            .client_session_for(
                client.id,
                SessionChannel::Cabinet,
                &RequestMeta::default(),
                false,
            )
            .await
            .unwrap()
            .token
    }

    #[tokio::test]
    async fn перебор_промокодов_с_одного_адреса_останавливается_для_новых_аккаунтов() {
        let Some((db, state)) = promo_state().await else {
            return;
        };
        sn_modules::catalog::promo::create(
            &state.services.module_context(),
            &serde_json::from_value(json!({"code": "REAL20", "kind": "percent", "percent": 20}))
                .unwrap(),
            None,
        )
        .await
        .unwrap();
        let attacker: SocketAddr = "203.0.113.7:40000".parse().unwrap();
        let other: SocketAddr = "203.0.113.8:40000".parse().unwrap();
        for account in 0..3 {
            let token = session(&state, 9_000 + account).await;
            let bearer = format!("Bearer {token}");
            for n in 0..10 {
                let (status, _, body) = send_from(
                    &state,
                    Some(attacker),
                    Method::POST,
                    "/api/client/m/promo/check",
                    &[("authorization", &bearer)],
                    Some(json!({ "code": format!("GUESS{account}X{n}") })),
                )
                .await;
                assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
            }
        }
        let token = session(&state, 9_100).await;
        let bearer = format!("Bearer {token}");
        let (status, _, body) = send_from(
            &state,
            Some(attacker),
            Method::POST,
            "/api/client/m/promo/check",
            &[("authorization", &bearer)],
            Some(json!({"code": "REAL20"})),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
        let (status, _, body) = send_from(
            &state,
            Some(other),
            Method::POST,
            "/api/client/m/promo/check",
            &[("authorization", &bearer)],
            Some(json!({"code": "REAL20"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        db.cleanup().await;
    }

    async fn real_code(state: &AppState) {
        sn_modules::catalog::promo::create(
            &state.services.module_context(),
            &serde_json::from_value(json!({"code": "REAL20", "kind": "percent", "percent": 20}))
                .unwrap(),
            None,
        )
        .await
        .unwrap();
    }

    async fn guess_from(
        state: &AppState,
        peer: SocketAddr,
        forwarded: Option<&str>,
        bearer: &str,
        code: &str,
    ) -> StatusCode {
        let mut headers = vec![("authorization", bearer)];
        if let Some(forwarded) = forwarded {
            headers.push(("x-forwarded-for", forwarded));
        }
        send_from(
            state,
            Some(peer),
            Method::POST,
            "/api/client/m/promo/check",
            &headers,
            Some(json!({ "code": code })),
        )
        .await
        .0
    }

    async fn exhaust_address(
        state: &AppState,
        peer: SocketAddr,
        forwarded: Option<&str>,
        first_account: i64,
    ) {
        let accounts = sn_modules::catalog::promo::ADDRESS_FAILURE_LIMIT
            / sn_modules::catalog::promo::FAILURE_LIMIT;
        for account in 0..i64::from(accounts) {
            let bearer = format!("Bearer {}", session(state, first_account + account).await);
            for n in 0..sn_modules::catalog::promo::FAILURE_LIMIT {
                assert_eq!(
                    guess_from(
                        state,
                        peer,
                        forwarded,
                        &bearer,
                        &format!("GUESS{account}X{n}")
                    )
                    .await,
                    StatusCode::UNPROCESSABLE_ENTITY
                );
            }
        }
    }

    #[tokio::test]
    async fn лимит_промокодов_считается_по_адресу_клиента_за_доверенным_прокси() {
        let Some((db, state)) = promo_state().await else {
            return;
        };
        real_code(&state).await;
        let proxy: SocketAddr = "127.0.0.1:41000".parse().unwrap();
        exhaust_address(&state, proxy, Some("203.0.113.7"), 9_200).await;
        let bearer = format!("Bearer {}", session(&state, 9_300).await);
        assert_eq!(
            guess_from(&state, proxy, Some("203.0.113.7"), &bearer, "REAL20").await,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            guess_from(&state, proxy, Some("203.0.113.8"), &bearer, "REAL20").await,
            StatusCode::OK,
            "сосед за тем же прокси не ограничен"
        );
        let direct: SocketAddr = "203.0.113.7:40000".parse().unwrap();
        assert_eq!(
            guess_from(&state, direct, Some("198.51.100.77"), &bearer, "REAL20").await,
            StatusCode::TOO_MANY_REQUESTS,
            "заголовок от недоверенного адреса не меняет адрес клиента"
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn расчёт_корзины_считает_неверные_промокоды_по_адресу_клиента() {
        use sn_domain::money::Stars;
        use sn_domain::tariff::{CurrencyPrices, DevicePolicy};
        use sn_domain::traffic::TrafficPolicy;
        use sn_store::{NewCategory, NewPeriod, NewTariff};
        let Some((db, state)) = promo_state().await else {
            return;
        };
        real_code(&state).await;
        let tariff = {
            let mut conn = state.services.store().conn().await.unwrap();
            let category = conn
                .tariffs()
                .create_category(&NewCategory {
                    name: "Стандарт".into(),
                    sort: 0,
                    visible: true,
                })
                .await
                .unwrap();
            conn.tariffs()
                .create(&NewTariff {
                    category_id: category.id,
                    name: "Стандарт".into(),
                    description: None,
                    on_sale: true,
                    sort: 0,
                    devices: DevicePolicy {
                        included: 3,
                        extra: None,
                    },
                    traffic: TrafficPolicy::UNLIMITED,
                    squads: Vec::new(),
                    external_squad: None,
                    periods: vec![NewPeriod {
                        days: 30,
                        prices: CurrencyPrices {
                            usd: Some(299),
                            rub: Some(19_900),
                        },
                        stars: Some(Stars(150)),
                        sort: 0,
                        active: true,
                    }],
                })
                .await
                .unwrap()
        };
        let proxy: SocketAddr = "127.0.0.1:41000".parse().unwrap();
        exhaust_address(&state, proxy, Some("203.0.113.40"), 9_400).await;
        let bearer = format!("Bearer {}", session(&state, 9_500).await);
        let cart = json!({
            "tariff_id": tariff.id,
            "period_id": tariff.periods[0].id,
            "promo_code": "REAL20",
        });
        let quote = |forwarded: &'static str| {
            let state = state.clone();
            let bearer = bearer.clone();
            let cart = cart.clone();
            async move {
                send_from(
                    &state,
                    Some(proxy),
                    Method::POST,
                    "/api/client/quote",
                    &[("authorization", &bearer), ("x-forwarded-for", forwarded)],
                    Some(cart),
                )
                .await
            }
        };
        let (status, _, body) = quote("203.0.113.40").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(
            body["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("Слишком много")),
            "{body}"
        );
        let (status, _, body) = quote("203.0.113.41").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["quote"]["total"]["minor"], json!(239), "{body}");
        db.cleanup().await;
    }

    #[tokio::test]
    async fn клиенты_за_прокси_на_loopback_не_делят_лимит_входа() {
        let Some((db, state)) = state().await else {
            return;
        };
        {
            let mut conn = state.services.store().conn().await.unwrap();
            conn.settings()
                .set_as(keys::BOT_USERNAME, &"stealth_vpn_bot", None)
                .await
                .unwrap();
        }
        let proxy: SocketAddr = "127.0.0.1:41000".parse().unwrap();
        let limit = sn_services::limits::rules::TELEGRAM_START_IP.limit;
        for i in 0..=limit {
            let forwarded = format!("203.0.113.{i}");
            let (status, _, body) = send_from(
                &state,
                Some(proxy),
                Method::POST,
                "/api/client/auth/telegram",
                &[("x-forwarded-for", &forwarded)],
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{forwarded}: {body}");
        }
        for attempt in 0..=limit {
            let (status, _, body) = send_from(
                &state,
                Some(proxy),
                Method::POST,
                "/api/client/auth/telegram",
                &[("x-forwarded-for", "198.51.100.7")],
                None,
            )
            .await;
            let expected = if attempt < limit {
                StatusCode::OK
            } else {
                StatusCode::TOO_MANY_REQUESTS
            };
            assert_eq!(status, expected, "{attempt}: {body}");
        }
        db.cleanup().await;
    }

    #[tokio::test]
    async fn мини_приложение_получает_токен_без_куки() {
        let Some((db, state)) = state().await else {
            return;
        };
        let (status, headers, body) = send(
            &state,
            Method::POST,
            "/api/client/auth/miniapp",
            &[],
            Some(json!({ "init_data": init_data(&state) })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(
            headers.get(header::SET_COOKIE).is_none(),
            "мини-приложение не должно получать куку: {:?}",
            headers.get(header::SET_COOKIE)
        );
        let token = body["token"].as_str().unwrap().to_string();
        let bearer = format!("Bearer {token}");
        let (status, _, me) = send(
            &state,
            Method::GET,
            "/api/client/me",
            &[("authorization", &bearer)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{me}");
        assert_eq!(me["channel"], "miniapp");
        let cookie = format!("sn_client={token}");
        let (status, _, _) = send(
            &state,
            Method::GET,
            "/api/client/me",
            &[("cookie", &cookie)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn кука_кабинета_не_закрывает_обращение_по_запросу_с_чужого_сайта() {
        let Some((db, state)) = state().await else {
            return;
        };
        let (client, token) = cabinet_client(&state).await;
        let ticket = open_ticket(&state, client).await;
        let path = format!("/api/client/support/tickets/{}/close", ticket.0);
        let cookie = format!("sn_client={token}");
        for headers in [
            vec![
                ("cookie", cookie.as_str()),
                ("origin", "https://evil.example"),
                ("sec-fetch-site", "cross-site"),
            ],
            vec![("cookie", cookie.as_str()), ("origin", "null")],
            vec![
                ("cookie", cookie.as_str()),
                ("origin", "https://shop.example.com"),
                ("sec-fetch-site", "same-site"),
            ],
        ] {
            let (status, _, body) = send(&state, Method::POST, &path, &headers, None).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{headers:?}: {body}");
            assert_eq!(body["error"]["code"], "auth.cross_site");
            assert_eq!(
                ticket_status(&state, client, ticket).await,
                TicketStatus::Open
            );
        }
        let (status, _, body) = send(
            &state,
            Method::POST,
            &path,
            &[
                ("cookie", &cookie),
                ("origin", PANEL),
                ("sec-fetch-site", "same-origin"),
            ],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            ticket_status(&state, client, ticket).await,
            TicketStatus::Closed
        );
        let second = open_ticket(&state, client).await;
        let bearer = format!("Bearer {token}");
        let (status, _, body) = send(
            &state,
            Method::POST,
            &format!("/api/client/support/tickets/{}/close", second.0),
            &[
                ("authorization", &bearer),
                ("origin", "https://evil.example"),
                ("sec-fetch-site", "cross-site"),
            ],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, _, body) = send(
            &state,
            Method::GET,
            "/api/client/support/tickets",
            &[("cookie", &cookie), ("sec-fetch-site", "cross-site")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        db.cleanup().await;
    }

    #[tokio::test]
    async fn выход_по_куке_с_чужого_сайта_не_закрывает_сессию() {
        let Some((db, state)) = state().await else {
            return;
        };
        let (_, token) = cabinet_client(&state).await;
        let cookie = format!("sn_client={token}");
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/client/auth/logout",
            &[
                ("cookie", &cookie),
                ("origin", "https://evil.example"),
                ("sec-fetch-site", "cross-site"),
            ],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(state.services.auth().client_session(&token).await.is_ok());
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/client/auth/logout",
            &[("cookie", &cookie), ("sec-fetch-site", "same-origin")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(state.services.auth().client_session(&token).await.is_err());
        db.cleanup().await;
    }

    #[tokio::test]
    async fn кука_админа_не_работает_для_изменений_с_чужого_сайта() {
        let Some((db, state)) = state().await else {
            return;
        };
        state
            .services
            .auth()
            .upsert_owner("owner@example.com", PASSWORD)
            .await
            .unwrap();
        let login = state
            .services
            .auth()
            .admin_login("owner@example.com", PASSWORD, None, &RequestMeta::default())
            .await
            .unwrap();
        let cookie = format!("sn_admin={}", login.token);
        let (status, _, body) = send(
            &state,
            Method::POST,
            "/api/admin/auth/2fa/setup",
            &[
                ("cookie", &cookie),
                ("origin", "https://evil.example"),
                ("sec-fetch-site", "cross-site"),
            ],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["error"]["code"], "auth.cross_site");
        let (status, _, body) = send(
            &state,
            Method::POST,
            "/api/admin/auth/2fa/setup",
            &[("cookie", &cookie), ("sec-fetch-site", "same-origin")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, _, body) = send(
            &state,
            Method::GET,
            "/api/admin/me",
            &[("cookie", &cookie), ("sec-fetch-site", "cross-site")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        db.cleanup().await;
    }
}
