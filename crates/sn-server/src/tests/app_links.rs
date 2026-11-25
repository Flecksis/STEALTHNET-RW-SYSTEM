use super::*;
use crate::routes::public_app::PATH as OPEN;
use sn_domain::ids::{RemnaUserId, SubscriptionId};
use sn_domain::subscription::{AutoRenewState, RemnaLink, SubscriptionKind};
use sn_remnawave::fake::{Fault, sample_user};
use sn_store::{NewSession, NewSubscription, SessionChannel, SessionScope};
use std::sync::Mutex;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

const FIRST: &str = "0123456789abcdef0123456789abcdef";
const SECOND: &str = "fedcba9876543210fedcba9876543210";

async fn subscription(h: &Http, client: ClientId, remna: i64) -> SubscriptionId {
    let user = sample_user(remna, &format!("sn_{remna}"));
    h.remna.insert_user(user.clone());
    let mut conn = h.services().store().conn().await.unwrap();
    let subscription = conn
        .subscriptions()
        .create(&NewSubscription {
            client_id: client,
            tariff_id: None,
            kind: SubscriptionKind::Paid,
            extra_devices: 0,
            autorenew: AutoRenewState::default(),
        })
        .await
        .unwrap();
    conn.subscriptions()
        .link_remna(
            subscription.id,
            &RemnaLink {
                user_id: RemnaUserId(remna),
                username: user.username,
                short_uuid: Some(user.short_uuid),
            },
        )
        .await
        .unwrap();
    subscription.id
}

async fn session(
    h: &Http,
    client: ClientId,
    channel: SessionChannel,
    scope: SessionScope,
) -> String {
    let token = format!("{channel:?}-{scope:?}-{}", client.0);
    let mut conn = h.services().store().conn().await.unwrap();
    conn.sessions()
        .create_client(
            client,
            channel,
            scope,
            &NewSession {
                token_hash: sn_services::auth::token_hash(&token),
                expires_at: Utc::now() + Duration::days(1),
                ip: None,
                user_agent: None,
            },
        )
        .await
        .unwrap();
    token
}

async fn mini(h: &Http, client: ClientId) -> String {
    session(h, client, SessionChannel::Miniapp, SessionScope::Full).await
}

fn path(id: SubscriptionId) -> String {
    format!("/api/client/subscriptions/{}/app-link", id.0)
}

fn happ() -> Value {
    json!({ "platform": "ios", "app": "happ", "step": 1 })
}

async fn issue(h: &Http, id: SubscriptionId, token: &str) -> String {
    let (status, body) = h.write(Method::POST, &path(id), token, happ()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["token"].as_str().unwrap().to_string()
}

async fn open(h: &Http, token: &str, holder: &str) -> (StatusCode, HeaderMap, Value) {
    h.send(
        Method::POST,
        OPEN,
        None,
        Some(json!({ "token": token, "holder": holder })),
    )
    .await
}

#[tokio::test]
async fn кнопка_мини_приложения_получает_одноразовую_ссылку_на_приложение() {
    let Some(h) = http().await else { return };
    let client = h.client(5_093_118_931).await;
    let id = subscription(&h, client, 931).await;
    let token = mini(&h, client).await;

    let (status, _, body) = h.send(Method::POST, &path(id), None, Some(happ())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let stranger = h.client(5_093_118_932).await;
    let foreign = mini(&h, stranger).await;
    let (status, body) = h.write(Method::POST, &path(id), &foreign, happ()).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    for bad in [
        json!({ "platform": "ios", "app": "happ", "step": 1, "button": 0 }),
        json!({ "platform": "ios", "app": "javascript:alert(1)", "step": 1 }),
        json!({ "platform": "ios", "app": "happ", "step": 0, "button": 0 }),
        json!({ "platform": "apple_tv", "app": "happ", "step": 1 }),
    ] {
        let (status, body) = h.write(Method::POST, &path(id), &token, bad.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {body}");
        assert_eq!(body["error"]["code"], "app_link.no_app", "{bad}");
    }
    for bad in [
        json!({ "platform": "browser", "app": "happ", "step": 1 }),
        json!({ "link": "https://evil.example" }),
    ] {
        let (status, body) = h.write(Method::POST, &path(id), &token, bad.clone()).await;
        assert!(status.is_client_error(), "{bad}: {status} {body}");
        assert_eq!(body["error"]["code"], "validation.invalid_body", "{bad}");
    }

    let (status, headers, issued) = h
        .send(Method::POST, &path(id), Some(&token), Some(happ()))
        .await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert!(!issued.to_string().contains("sub.example.com"), "{issued}");
    let link = issued["token"].as_str().unwrap();
    assert_eq!(link.len(), 64);

    let (status, headers, opened) = open(&h, link, FIRST).await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        opened["link"],
        "happ://add/https://sub.example.com/short931"
    );
    assert_eq!(
        opened["subscription_url"],
        "https://sub.example.com/short931"
    );
    assert_eq!(opened["name"], "Happ");
    assert_eq!(opened["locale"], "ru");
    assert_eq!(opened["expires_in"], 600);
    assert_eq!(opened["install"][0]["label"]["en"], "App Store");
    let (status, _, again) = open(&h, link, FIRST).await;
    assert_eq!((status, &again), (StatusCode::OK, &opened));
    let (status, _, body) = open(&h, link, SECOND).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "app_link.taken");
    assert!(!body.to_string().contains("sub.example.com"), "{body}");
    let (status, _, body) = open(&h, &"a".repeat(64), FIRST).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "app_link.expired");
    let (status, _, _) = h.send(Method::GET, OPEN, None, None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

    h.clock.set(start() + Duration::minutes(10));
    let (status, _, body) = open(&h, link, FIRST).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "app_link.expired");
    assert_eq!(
        body["error"]["message"],
        "Ссылка устарела. Вернитесь в Telegram и нажмите кнопку ещё раз"
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn ссылку_на_приложение_выдаёт_та_же_сессия_что_видит_подписку() {
    let Some(h) = http().await else { return };
    let client = h.client(5_093_118_933).await;
    let id = subscription(&h, client, 933).await;
    let view = session(
        &h,
        client,
        SessionChannel::Cabinet,
        SessionScope::SubscriptionView,
    )
    .await;
    let (status, body) = h
        .get(&format!("/api/client/subscriptions/{}", id.0), &view)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["subscription_url"], "https://sub.example.com/short933");
    let link = issue(&h, id, &view).await;
    assert_eq!(open(&h, &link, FIRST).await.0, StatusCode::OK);

    let pending = session(
        &h,
        client,
        SessionChannel::Cabinet,
        SessionScope::PasswordChange,
    )
    .await;
    let (status, body) = h.write(Method::POST, &path(id), &pending, happ()).await;
    assert!(status.is_client_error(), "{status} {body}");
    assert_ne!(status, StatusCode::NOT_FOUND);

    let admin = h.owner().await;
    let (status, body) = h.write(Method::POST, &path(id), &admin, happ()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn страница_перехода_говорит_с_клиентом_на_его_языке_и_без_подробностей_сбоя() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/languages",
            &owner,
            json!({ "english": true, "default": "ru" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.services().set_remna(Some(h.remna.clone()));
    let client = h.client(5_093_118_934).await;
    let id = subscription(&h, client, 934).await;
    let token = mini(&h, client).await;
    let link = issue(&h, id, &token).await;
    let ask = async |lang: &str, holder: &str| {
        let request = Request::builder()
            .method(Method::POST)
            .uri(OPEN)
            .header(crate::lang::HEADER, lang)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "token": link, "holder": holder }).to_string(),
            ))
            .unwrap();
        let response = router(h.state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        (status, body_json(response).await)
    };
    for (lang, text) in [
        (
            "ru",
            "Сервис VPN сейчас недоступен. Попробуйте через несколько минут.",
        ),
        (
            "en",
            "The VPN service is unavailable right now. Try again in a few minutes.",
        ),
    ] {
        h.remna.fail_on("get_user", Fault::Redirected);
        let (status, body) = ask(lang, FIRST).await;
        assert!(status.is_server_error(), "{status} {body}");
        assert_eq!(body["error"]["message"], text);
    }
    let (status, body) = ask("en", FIRST).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = ask("en", SECOND).await;
    assert_eq!(
        body["error"]["message"],
        "This link is already open in another window. Go back to Telegram and tap the button again."
    );
    let (_, body) = ask("ru", SECOND).await;
    assert_eq!(
        body["error"]["message"],
        "Эту ссылку уже открыли в другом окне. Вернитесь в Telegram и нажмите кнопку ещё раз"
    );
    h.db.cleanup().await;
}

struct Lines(Arc<Mutex<Vec<String>>>);

struct Line(String);

impl Visit for Line {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={value:?}", field.name()));
    }
}

impl Lines {
    fn add(&self, line: Line) {
        self.0.lock().unwrap().push(line.0);
    }
}

impl Subscriber for Lines {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, span: &Attributes<'_>) -> Id {
        let mut line = Line(span.metadata().name().to_string());
        span.record(&mut line);
        self.add(line);
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, values: &Record<'_>) {
        let mut line = Line(String::new());
        values.record(&mut line);
        self.add(line);
    }

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut line = Line(event.metadata().target().to_string());
        event.record(&mut line);
        self.add(line);
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

#[tokio::test]
async fn ни_ссылка_ни_подписка_не_попадают_в_журнал() {
    let Some(h) = http().await else { return };
    let client = h.client(5_093_118_935).await;
    let id = subscription(&h, client, 935).await;
    let token = mini(&h, client).await;
    let lines = Arc::new(Mutex::new(Vec::new()));
    let second = tracing::Dispatch::new(Lines(Arc::default()));
    let journal = tracing::Dispatch::new(Lines(lines.clone()));
    let guard = tracing::dispatcher::set_default(&journal);
    let link = issue(&h, id, &token).await;
    assert_eq!(open(&h, &link, FIRST).await.0, StatusCode::OK);
    assert_eq!(
        open(&h, &link, SECOND).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    h.remna.fail_on("get_user", Fault::Unavailable);
    assert!(open(&h, &link, FIRST).await.0.is_server_error());
    h.clock.set(start() + Duration::minutes(10));
    assert_eq!(
        open(&h, &link, FIRST).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    drop(guard);
    drop(second);
    let lines = lines.lock().unwrap().clone();
    assert!(
        lines.iter().any(|line| line.contains(OPEN)),
        "журнал не собран: {lines:?}"
    );
    for line in &lines {
        for secret in [
            link.as_str(),
            FIRST,
            SECOND,
            "sub.example.com",
            "short935",
            "happ://",
        ] {
            assert!(!line.contains(secret), "{secret}: {line}");
        }
    }
    h.db.cleanup().await;
}

#[tokio::test]
async fn страница_перехода_не_перенаправляет_и_не_встраивается() {
    let (state, dir) = web_state().await;
    for path in [
        "/open-app",
        "/open-app?url=https://evil.example",
        "/open-app?link=happ://add/https://evil.example",
        "/open-app/",
    ] {
        let (status, headers, body) = fetch(&state, Method::GET, path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(!headers.contains_key(header::LOCATION), "{path}");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY", "{path}");
        assert_eq!(headers[header::REFERRER_POLICY], "no-referrer", "{path}");
        assert!(!body.contains("evil.example"), "{path}: {body}");
        assert!(body.contains("noindex"), "{path}: {body}");
    }
    std::fs::remove_dir_all(dir).ok();
}
