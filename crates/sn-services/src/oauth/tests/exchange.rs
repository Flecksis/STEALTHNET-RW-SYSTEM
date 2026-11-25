use super::*;
use sn_domain::client::Languages;
use sn_domain::lang;
use sn_modules::catalog::antibot::AntibotModule;
use sn_store::{Store, StoreOptions};
use std::time::Instant;

const SLOW: Delay = Delay::from_secs(4);
const SHORT: Delay = Delay::from_millis(1500);
const PATIENCE: Delay = Delay::from_secs(10);

impl Harness {
    async fn copy(&self, connections: u32, acquire_timeout: Delay) -> (Store, Services) {
        let store = Store::connect(
            &sn_store::testing::test_database_url().unwrap(),
            StoreOptions {
                max_connections: connections,
                acquire_timeout,
                search_path: Some(self.db.schema().to_string()),
                application_name: self.db.schema().to_string(),
                ..StoreOptions::default()
            },
        )
        .await
        .unwrap();
        let services = Services::new(ServiceDeps {
            store: store.clone(),
            payments: Arc::new(ProviderRegistry::builtin()),
            modules: Arc::new(
                ModuleRegistry::new(vec![
                    Arc::new(OauthModule),
                    Arc::new(UtmModule),
                    Arc::new(AntibotModule),
                ])
                .unwrap(),
            ),
            secrets: self.services.secrets().cloned(),
            clock: self.clock.clone(),
            env: EnvFallbacks {
                public_url: Some(PANEL.into()),
                ..EnvFallbacks::default()
            },
        });
        services.reload().await.unwrap();
        services.set_oauth_endpoints(self.services.oauth_endpoints().as_ref().clone());
        services.set_captcha_endpoints(self.services.captcha_endpoints().as_ref().clone());
        (store, services)
    }

    async fn one_connection(&self) -> (Store, Services) {
        self.copy(1, Delay::from_secs(2)).await
    }

    async fn slow(&self, provider: Provider, flow: &Flow, sub: &str, delay: Delay) {
        let extra =
            serde_json::json!({ "email": format!("{sub}@gmail.com"), "email_verified": true });
        let token = self.token(provider, flow, sub, extra);
        self.reply_after(provider, flow, token, delay).await;
    }

    async fn exchanges(&self, provider: Provider, flow: &Flow) -> usize {
        self.server(provider)
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| {
                r.url.path() == "/token"
                    && String::from_utf8_lossy(&r.body).contains(&format!("code={}", flow.code))
            })
            .count()
    }

    async fn key_requests(&self, provider: Provider) -> usize {
        self.server(provider)
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path().starts_with("/late-keys/"))
            .count()
    }

    async fn asked_times(&self, provider: Provider, flow: &Flow, times: usize) {
        let deadline = Instant::now() + PATIENCE;
        while self.exchanges(provider, flow).await < times {
            assert!(
                Instant::now() < deadline,
                "запрос к {} не ушёл: вход ждёт соединения с базой",
                provider.title()
            );
            tokio::time::sleep(Delay::from_millis(20)).await;
        }
    }

    async fn asked(&self, provider: Provider, flow: &Flow) {
        self.asked_times(provider, flow, 1).await;
    }

    async fn idle(&self) {
        let deadline = Instant::now() + PATIENCE;
        while self.services.pause_gate().active() > 0 {
            assert!(Instant::now() < deadline, "возврат не закончился");
            tokio::time::sleep(Delay::from_millis(20)).await;
        }
    }

    async fn age(&self, flow: &Flow, seconds: i64) {
        let mut conn = self.services.store().conn().await.unwrap();
        let aged = sqlx::query(
            "UPDATE oauth_requests SET exchange_at = exchange_at - make_interval(secs => $2) \
             WHERE state_hash = $1 AND status = 'exchanging'",
        )
        .bind(token_hash(&flow.state))
        .bind(seconds as f64)
        .execute(conn.conn())
        .await
        .unwrap();
        assert_eq!(aged.rows_affected(), 1);
    }

    async fn status(&self, flow: &Flow) -> String {
        let mut conn = self.services.store().conn().await.unwrap();
        sqlx::query_scalar("SELECT status FROM oauth_requests WHERE state_hash = $1")
            .bind(token_hash(&flow.state))
            .fetch_one(conn.conn())
            .await
            .unwrap()
    }

    async fn sessions_count(&self) -> i64 {
        let mut conn = self.services.store().conn().await.unwrap();
        sqlx::query_scalar("SELECT count(*) FROM client_sessions")
            .fetch_one(conn.conn())
            .await
            .unwrap()
    }
}

fn returning(
    services: &Services,
    provider: Provider,
    flow: &Flow,
) -> impl Future<Output = ServiceResult<CallbackOutcome>> + Send + 'static {
    let services = services.clone();
    let params = flow.params(None);
    let browser = flow.start.browser.clone();
    async move {
        services
            .oauth()
            .callback(provider, &params, Some(&browser))
            .await
    }
}

async fn finishing(
    services: &Services,
    flow: &Flow,
    captcha: Option<&str>,
) -> ServiceResult<OauthFinish> {
    services
        .oauth()
        .finish_with(Some(&flow.start.browser), &meta(), captcha)
        .await
}

async fn base_is_free(store: &Store) {
    let started = Instant::now();
    store.ping().await.unwrap_or_else(|err| {
        panic!("пока провайдер входа отвечает, база недоступна другим запросам: {err}")
    });
    assert!(
        started.elapsed() < Delay::from_secs(1),
        "запрос ждал соединения {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn пока_google_и_apple_отвечают_соединение_с_базой_свободно() {
    let Some(h) = harness().await else { return };
    let owner = h.telegram_client(5_093_118_500).await;
    let principal = h.cabinet(owner).await;
    let google = h.login_flow(Provider::Google).await;
    let apple = h.login_flow(Provider::Apple).await;
    let link = Harness::flow(
        h.ops()
            .start_link(&principal, Provider::Google, &meta())
            .await
            .unwrap(),
    );
    h.slow(Provider::Google, &google, "9101", SLOW).await;
    h.slow(Provider::Apple, &apple, "9102", SLOW).await;
    h.slow(Provider::Google, &link, "9103", SLOW).await;
    let (store, services) = h.one_connection().await;
    let flows = [
        (Provider::Google, &google),
        (Provider::Apple, &apple),
        (Provider::Google, &link),
    ];
    let running: Vec<_> = flows
        .iter()
        .map(|(provider, flow)| tokio::spawn(returning(&services, *provider, flow)))
        .collect();
    for (provider, flow) in flows {
        h.asked(provider, flow).await;
        base_is_free(&store).await;
    }
    for back in &running {
        assert!(!back.is_finished(), "провайдер ответил слишком рано");
    }
    let mut purposes = Vec::new();
    for back in running {
        purposes.push(back.await.unwrap().unwrap().purpose);
    }
    assert_eq!(purposes, [Purpose::Login, Purpose::Login, Purpose::Link]);
    for flow in [&google, &apple] {
        assert!(confirmed(finishing(&services, flow, None).await).created);
    }
    assert_eq!(
        finishing(&services, &link, None).await.unwrap(),
        OauthFinish::Linked {
            provider: Provider::Google
        }
    );
    assert!(
        h.identities(owner)
            .await
            .iter()
            .any(|i| i.kind == IdentityKind::Google && i.value == "9103")
    );
    assert_eq!(h.clients_count().await, 3);
    store.close().await;
}

#[tokio::test]
async fn пока_google_отдаёт_ключи_соединение_с_базой_свободно() {
    let Some(h) = harness().await else { return };
    Mock::given(method("GET"))
        .and(path_regex("^/late-keys/"))
        .respond_with(ResponseTemplate::new(200).set_delay(SLOW).set_body_json(
            serde_json::json!({
                "keys": [{ "kty": "RSA", "kid": KID, "use": "sig", "alg": "RS256", "n": IDP_N.trim(), "e": "AQAB" }]
            }),
        ))
        .mount(&h.google)
        .await;
    let (store, services) = h.one_connection().await;
    let mut endpoints = h.services.oauth_endpoints().as_ref().clone();
    endpoints.google.jwks = format!(
        "{}/late-keys/{}",
        h.google.uri(),
        uuid::Uuid::new_v4().simple()
    );
    services.set_oauth_endpoints(endpoints);
    let flow = h.login_flow(Provider::Google).await;
    h.slow(Provider::Google, &flow, "9201", Delay::ZERO).await;
    let running = tokio::spawn(returning(&services, Provider::Google, &flow));
    let deadline = Instant::now() + PATIENCE;
    while h.key_requests(Provider::Google).await == 0 {
        assert!(Instant::now() < deadline, "ключи Google не запрошены");
        tokio::time::sleep(Delay::from_millis(20)).await;
    }
    base_is_free(&store).await;
    assert!(!running.is_finished(), "ключи пришли слишком рано");
    running.await.unwrap().unwrap();
    assert!(confirmed(finishing(&services, &flow, None).await).created);
    store.close().await;
}

#[tokio::test]
async fn возвраты_разом_при_медленном_google_не_ждут_друг_друга() {
    const PARALLEL: usize = 20;
    let Some(h) = harness().await else { return };
    let mut flows = Vec::new();
    for n in 0..PARALLEL {
        let start = h
            .ops()
            .start_login(
                Provider::Google,
                None,
                &RequestMeta {
                    ip: Some(format!("198.51.100.{}", n + 1)),
                    user_agent: None,
                },
            )
            .await
            .unwrap();
        let flow = Harness::flow(start);
        h.slow(Provider::Google, &flow, &format!("93{n:02}"), SHORT)
            .await;
        flows.push(flow);
    }
    let (store, services) = h.copy(1, Delay::from_secs(8)).await;
    let started = Instant::now();
    let running: Vec<_> = flows
        .iter()
        .map(|flow| tokio::spawn(returning(&services, Provider::Google, flow)))
        .collect();
    for back in running {
        back.await.unwrap().unwrap();
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < SHORT * (PARALLEL as u32) / 2,
        "возвраты ждали друг друга: {elapsed:?}"
    );
    for flow in &flows {
        assert!(confirmed(finishing(&services, flow, None).await).created);
        assert_eq!(h.exchanges(Provider::Google, flow).await, 1);
    }
    assert_eq!(h.clients_count().await, PARALLEL as i64);
    store.close().await;
}

#[tokio::test]
async fn одновременный_возврат_с_тем_же_state_заводит_один_аккаунт_и_один_вход() {
    let Some(h) = harness().await else { return };
    let flow = h.login_flow(Provider::Google).await;
    h.slow(Provider::Google, &flow, "9401", SHORT).await;
    let (store, copy) = h.copy(4, Delay::from_secs(8)).await;
    let mut tabs = Vec::new();
    for n in 0..10 {
        let services = if n % 2 == 0 {
            h.services.clone()
        } else {
            copy.clone()
        };
        let back = returning(&services, Provider::Google, &flow);
        let browser = flow.start.browser.clone();
        tabs.push(tokio::spawn(async move {
            let outcome = back.await.unwrap();
            assert_eq!(outcome.purpose, Purpose::Login);
            services.oauth().finish(Some(&browser), &meta()).await
        }));
    }
    let mut logins = Vec::new();
    let mut refusals = Vec::new();
    for tab in tabs {
        match tab.await.unwrap() {
            Ok(OauthFinish::Confirmed { login }) => logins.push(login),
            Ok(other) => panic!("ожидался вход, пришло {other:?}"),
            Err(err) => refusals.push(err.code()),
        }
    }
    assert_eq!(logins.len(), 1, "{refusals:?}");
    assert!(logins[0].created);
    assert_eq!(refusals, ["oauth.no_request"; 9]);
    assert_eq!(h.exchanges(Provider::Google, &flow).await, 1);
    assert_eq!(h.clients_count().await, 1);
    assert_eq!(h.sessions_count().await, 1);
    h.back(Provider::Google, &flow, None).await;
    assert_eq!(refused(h.finish(&flow).await), "oauth.no_request");
    assert_eq!(h.exchanges(Provider::Google, &flow).await, 1);
    let mut conn = h.services.store().conn().await.unwrap();
    let registered: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'client.oauth_registered'",
    )
    .fetch_one(conn.conn())
    .await
    .unwrap();
    assert_eq!(registered, 1);
    drop(conn);
    store.close().await;
}

#[tokio::test]
async fn разные_входы_одного_человека_разом_заводят_один_аккаунт() {
    let Some(h) = harness().await else { return };
    let mut flows = Vec::new();
    for _ in 0..4 {
        let flow = h.login_flow(Provider::Google).await;
        h.slow(Provider::Google, &flow, "9451", SHORT).await;
        flows.push(flow);
    }
    let running: Vec<_> = flows
        .iter()
        .map(|flow| tokio::spawn(returning(&h.services, Provider::Google, flow)))
        .collect();
    for back in running {
        back.await.unwrap().unwrap();
    }
    let mut created = 0;
    for flow in &flows {
        created += usize::from(confirmed(h.finish(flow).await).created);
    }
    assert_eq!(created, 1);
    assert_eq!(h.clients_count().await, 1);
}

#[tokio::test]
async fn кабинет_дожидается_ответа_google_не_занимая_соединения() {
    let Some(h) = harness().await else { return };
    let flow = h.login_flow(Provider::Google).await;
    h.slow(Provider::Google, &flow, "9501", SLOW).await;
    let (store, services) = h.one_connection().await;
    let running = tokio::spawn(returning(&services, Provider::Google, &flow));
    h.asked(Provider::Google, &flow).await;
    let repeated = returning(&services, Provider::Google, &flow).await.unwrap();
    assert_eq!(repeated.purpose, Purpose::Login);
    let site = services.clone();
    let browser = flow.start.browser.clone();
    let waiting = tokio::spawn(async move { site.oauth().finish(Some(&browser), &meta()).await });
    tokio::time::sleep(SLOW / 4).await;
    assert!(!waiting.is_finished(), "кабинет не дождался ответа Google");
    assert!(!running.is_finished(), "Google ответил слишком рано");
    base_is_free(&store).await;
    running.await.unwrap().unwrap();
    assert!(confirmed(waiting.await.unwrap()).created);
    assert_eq!(h.exchanges(Provider::Google, &flow).await, 1);
    assert_eq!(h.clients_count().await, 1);
    assert_eq!(h.sessions_count().await, 1);
    store.close().await;
}

#[tokio::test]
async fn оборванный_браузером_возврат_доводит_вход_до_конца() {
    let Some(h) = harness().await else { return };
    for reload in [true, false] {
        let flow = h.login_flow(Provider::Google).await;
        let sub = format!("960{}", u8::from(reload));
        h.slow(Provider::Google, &flow, &sub, SHORT).await;
        let running = tokio::spawn(returning(&h.services, Provider::Google, &flow));
        h.asked(Provider::Google, &flow).await;
        running.abort();
        assert!(running.await.unwrap_err().is_cancelled());
        assert_eq!(h.services.pause_gate().active(), 1);
        if reload {
            h.back(Provider::Google, &flow, None).await;
        }
        let login = confirmed(h.finish(&flow).await);
        assert!(login.created, "{reload}");
        assert_eq!(h.exchanges(Provider::Google, &flow).await, 1, "{reload}");
        h.idle().await;
    }
    assert_eq!(h.clients_count().await, 2);
}

#[tokio::test]
async fn обмен_брошенный_упавшей_панелью_можно_повторить() {
    let Some(h) = harness().await else { return };
    let flow = h.login_flow(Provider::Google).await;
    h.slow(Provider::Google, &flow, "9701", Delay::ZERO).await;
    {
        let mut conn = h.services.store().conn().await.unwrap();
        sqlx::query(
            "UPDATE oauth_requests SET status = 'exchanging', exchange_at = clock_timestamp() WHERE state_hash = $1",
        )
        .bind(token_hash(&flow.state))
        .execute(conn.conn())
        .await
        .unwrap();
    }
    h.back(Provider::Google, &flow, None).await;
    assert_eq!(h.status(&flow).await, "exchanging");
    h.age(&flow, EXCHANGE_SECONDS - 20).await;
    h.back(Provider::Google, &flow, None).await;
    assert_eq!(h.status(&flow).await, "exchanging");
    assert_eq!(h.exchanges(Provider::Google, &flow).await, 0);
    h.age(&flow, 20).await;
    let asked = Instant::now();
    assert_eq!(refused(h.finish(&flow).await), "oauth.no_request");
    assert!(asked.elapsed() < Delay::from_secs(5), "кабинет ждал зря");
    assert_eq!(h.status(&flow).await, "exchanging");
    h.back(Provider::Google, &flow, None).await;
    assert_eq!(h.exchanges(Provider::Google, &flow).await, 1);
    assert!(confirmed(h.finish(&flow).await).created);
    assert_eq!(h.clients_count().await, 1);
}

#[tokio::test]
async fn опоздавший_ответ_google_не_открывает_вход_второй_раз() {
    let Some(h) = harness().await else { return };
    let flow = h.login_flow(Provider::Google).await;
    h.slow(Provider::Google, &flow, "9751", SLOW).await;
    let first = tokio::spawn(returning(&h.services, Provider::Google, &flow));
    h.asked(Provider::Google, &flow).await;
    tokio::time::sleep(SLOW / 2).await;
    h.age(&flow, EXCHANGE_SECONDS).await;
    let second = tokio::spawn(returning(&h.services, Provider::Google, &flow));
    h.asked_times(Provider::Google, &flow, 2).await;
    first.await.unwrap().unwrap();
    assert!(confirmed(h.finish(&flow).await).created);
    assert_eq!(h.status(&flow).await, "consumed");
    assert!(!second.is_finished(), "второй ответ пришёл слишком рано");
    second.await.unwrap().unwrap();
    assert_eq!(h.status(&flow).await, "consumed");
    assert_eq!(refused(h.finish(&flow).await), "oauth.no_request");
    assert_eq!(h.clients_count().await, 1);
    assert_eq!(h.sessions_count().await, 1);
}

#[tokio::test]
async fn опоздавший_отказ_google_не_мешает_начатому_заново_обмену() {
    let Some(h) = harness().await else { return };
    let flow = h.login_flow(Provider::Google).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains(format!("code={}", flow.code)))
        .respond_with(
            ResponseTemplate::new(400)
                .set_delay(SLOW)
                .set_body_json(serde_json::json!({ "error": "invalid_grant" })),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&h.google)
        .await;
    h.slow(Provider::Google, &flow, "9761", SLOW).await;
    let first = tokio::spawn(returning(&h.services, Provider::Google, &flow));
    h.asked(Provider::Google, &flow).await;
    tokio::time::sleep(SLOW / 2).await;
    h.age(&flow, EXCHANGE_SECONDS).await;
    let second = tokio::spawn(returning(&h.services, Provider::Google, &flow));
    h.asked_times(Provider::Google, &flow, 2).await;
    first.await.unwrap().unwrap();
    assert_eq!(h.status(&flow).await, "exchanging");
    assert!(!second.is_finished(), "второй ответ пришёл слишком рано");
    second.await.unwrap().unwrap();
    assert!(confirmed(h.finish(&flow).await).created);
    assert_eq!(h.clients_count().await, 1);
}

async fn break_identities(h: &Harness, writes: i64) {
    let mut conn = h.services.store().conn().await.unwrap();
    for statement in [
        "CREATE SEQUENCE broken_writes",
        &format!(
            "CREATE FUNCTION broken() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN IF nextval('broken_writes') <= {writes} THEN RAISE EXCEPTION 'сбой'; END IF; RETURN NEW; END $$"
        ),
        "CREATE TRIGGER broken BEFORE INSERT ON client_identities FOR EACH ROW EXECUTE FUNCTION broken()",
    ] {
        sqlx::query(statement).execute(conn.conn()).await.unwrap();
    }
}

async fn repair_identities(h: &Harness) {
    let mut conn = h.services.store().conn().await.unwrap();
    sqlx::query("DROP TRIGGER broken ON client_identities")
        .execute(conn.conn())
        .await
        .unwrap();
}

#[tokio::test]
async fn сбой_старого_обмена_не_сбивает_начатый_заново() {
    let Some(h) = harness().await else { return };
    let flow = h.login_flow(Provider::Google).await;
    h.slow(Provider::Google, &flow, "9781", SLOW * 3 / 2).await;
    let first = tokio::spawn(returning(&h.services, Provider::Google, &flow));
    h.asked(Provider::Google, &flow).await;
    tokio::time::sleep(SLOW * 3 / 4).await;
    h.age(&flow, EXCHANGE_SECONDS).await;
    let second = tokio::spawn(returning(&h.services, Provider::Google, &flow));
    h.asked_times(Provider::Google, &flow, 2).await;
    break_identities(&h, i64::MAX).await;
    assert!(first.await.unwrap().is_err());
    repair_identities(&h).await;
    assert_eq!(h.status(&flow).await, "exchanging");
    assert!(!second.is_finished(), "второй ответ пришёл слишком рано");
    second.await.unwrap().unwrap();
    assert!(confirmed(h.finish(&flow).await).created);
    assert_eq!(h.clients_count().await, 1);
}

#[tokio::test]
async fn сбой_базы_после_ответа_google_оставляет_запрос_для_повтора() {
    let Some(h) = harness().await else { return };
    let flow = h.login_flow(Provider::Google).await;
    h.slow(Provider::Google, &flow, "9801", Delay::ZERO).await;
    break_identities(&h, i64::MAX).await;
    assert!(
        returning(&h.services, Provider::Google, &flow)
            .await
            .is_err()
    );
    assert_eq!(h.status(&flow).await, "pending");
    assert_eq!(refused(h.finish(&flow).await), "oauth.no_request");
    assert_eq!(h.clients_count().await, 0);
    repair_identities(&h).await;
    h.back(Provider::Google, &flow, None).await;
    assert!(confirmed(h.finish(&flow).await).created);
    assert_eq!(h.clients_count().await, 1);
}

#[tokio::test]
async fn короткий_сбой_базы_не_теряет_ответ_google() {
    let Some(h) = harness().await else { return };
    let flow = h.login_flow(Provider::Google).await;
    h.slow(Provider::Google, &flow, "9851", Delay::ZERO).await;
    break_identities(&h, i64::from(WRITE_ATTEMPTS) - 1).await;
    h.back(Provider::Google, &flow, None).await;
    assert!(confirmed(h.finish(&flow).await).created);
    assert_eq!(h.exchanges(Provider::Google, &flow).await, 1);
    assert_eq!(h.clients_count().await, 1);
    assert_eq!(h.sessions_count().await, 1);
}

#[tokio::test]
async fn кабинет_не_дождавшийся_google_может_забрать_вход_позже() {
    let Some(h) = harness().await else { return };
    let flow = h.login_flow(Provider::Google).await;
    h.slow(Provider::Google, &flow, "9871", SLOW).await;
    let running = tokio::spawn(returning(&h.services, Provider::Google, &flow));
    h.asked(Provider::Google, &flow).await;
    let early = h
        .ops()
        .await_exchange(&flow.start.browser, Delay::from_millis(600))
        .await
        .unwrap_err();
    assert_eq!(early.code(), UNFINISHED);
    let hurried = h
        .ops()
        .finish_once(&flow.start.browser, &meta(), false)
        .await
        .unwrap_err();
    assert_eq!(hurried.code(), UNFINISHED);
    assert!(!running.is_finished(), "Google ответил слишком рано");
    running.await.unwrap().unwrap();
    assert!(confirmed(h.finish(&flow).await).created);
    assert_eq!(h.clients_count().await, 1);
}

#[tokio::test]
async fn занятая_база_не_обрывает_ожидание_кабинета() {
    let Some(h) = harness().await else { return };
    let flow = h.login_flow(Provider::Google).await;
    h.slow(Provider::Google, &flow, "9881", SLOW).await;
    let (store, services) = h.copy(1, Delay::from_millis(300)).await;
    let running = tokio::spawn(returning(&services, Provider::Google, &flow));
    h.asked(Provider::Google, &flow).await;
    let site = services.clone();
    let browser = flow.start.browser.clone();
    let waiting = tokio::spawn(async move { site.oauth().finish(Some(&browser), &meta()).await });
    tokio::time::sleep(SLOW / 8).await;
    let busy = loop {
        match store.conn().await {
            Ok(conn) => break conn,
            Err(_) => tokio::time::sleep(Delay::from_millis(20)).await,
        }
    };
    tokio::time::sleep(SLOW / 4).await;
    drop(busy);
    assert!(!waiting.is_finished(), "кабинет не дождался Google");
    assert!(!running.is_finished(), "Google ответил слишком рано");
    running.await.unwrap().unwrap();
    assert!(confirmed(waiting.await.unwrap()).created);
    store.close().await;
}

#[tokio::test]
async fn пауза_восстановления_ждёт_обмен_начатый_до_неё() {
    let Some(h) = harness().await else { return };
    let flow = h.login_flow(Provider::Google).await;
    h.slow(Provider::Google, &flow, "9891", SHORT).await;
    let gate = h.services.pause_gate().clone();
    let request = gate.try_enter().unwrap();
    let pausing = {
        let gate = gate.clone();
        tokio::spawn(async move { gate.pause(PATIENCE).await })
    };
    while !gate.is_paused() {
        tokio::time::sleep(Delay::from_millis(20)).await;
    }
    let running = tokio::spawn(returning(&h.services, Provider::Google, &flow));
    h.asked(Provider::Google, &flow).await;
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    drop(request);
    tokio::time::sleep(Delay::from_millis(200)).await;
    assert!(!pausing.is_finished(), "пауза не дождалась обмена");
    drop(pausing.await.unwrap());
    assert_eq!(h.status(&flow).await, "done");
    assert!(confirmed(h.finish(&flow).await).created);
}

#[tokio::test]
async fn привязка_сверяет_сессию_и_клиента_после_ответа_google() {
    let Some(h) = harness().await else { return };
    let gone = h.telegram_client(5_093_118_601).await;
    let blocked = h.telegram_client(5_093_118_602).await;
    let late = h.telegram_client(5_093_118_603).await;
    let mut flows = Vec::new();
    for (client, sub) in [(gone, "9901"), (blocked, "9902"), (late, "9903")] {
        let principal = h.cabinet(client).await;
        let flow = Harness::flow(
            h.ops()
                .start_link(&principal, Provider::Google, &meta())
                .await
                .unwrap(),
        );
        h.slow(Provider::Google, &flow, sub, SLOW).await;
        flows.push(flow);
    }
    let running: Vec<_> = flows
        .iter()
        .map(|flow| tokio::spawn(returning(&h.services, Provider::Google, flow)))
        .collect();
    for flow in &flows {
        h.asked(Provider::Google, flow).await;
    }
    {
        let mut conn = h.services.store().conn().await.unwrap();
        conn.sessions()
            .revoke_client_channel(gone, SessionChannel::Cabinet)
            .await
            .unwrap();
        conn.clients()
            .set_status(
                blocked,
                sn_domain::client::ClientStatus::Blocked,
                Some("проверка"),
            )
            .await
            .unwrap();
        let other = conn
            .clients()
            .find_by_identity(IdentityKind::Telegram, "5093118601")
            .await
            .unwrap()
            .unwrap();
        conn.clients()
            .add_identity(
                other.id,
                &NewIdentity::new(IdentityKind::Google, "9903", true).unwrap(),
            )
            .await
            .unwrap();
    }
    for back in &running {
        assert!(!back.is_finished(), "Google ответил слишком рано");
    }
    for back in running {
        assert_eq!(back.await.unwrap().unwrap().purpose, Purpose::Link);
    }
    let mut codes = Vec::new();
    for flow in &flows {
        codes.push(refused(h.finish(flow).await));
    }
    assert_eq!(
        codes,
        ["oauth.session_gone", "oauth.blocked", "oauth.taken"]
    );
    for client in [blocked, late] {
        assert!(
            h.identities(client)
                .await
                .iter()
                .all(|i| i.kind == IdentityKind::Telegram)
        );
    }
}

#[tokio::test]
async fn отложенный_вход_с_капчей_обходится_одним_соединением() {
    let Some(h) = limited(serde_json::json!({
        "site_minute": 1,
        "captcha": "turnstile",
        "captcha_site_key": "0x4AAAAAAAsite",
        "captcha_secret": "0x4AAAAAAAsecret-key",
        "captcha_signup": false,
    }))
    .await
    else {
        return;
    };
    let _verify = crate::scenario_tests::siteverify::siteverify(&h.services).await;
    signups_made(&h).await;
    let flow = h.login_flow(Provider::Google).await;
    h.slow(Provider::Google, &flow, "9951", SLOW).await;
    let (store, services) = h.one_connection().await;
    let running = tokio::spawn(returning(&services, Provider::Google, &flow));
    h.asked(Provider::Google, &flow).await;
    base_is_free(&store).await;
    running.await.unwrap().unwrap();
    assert_eq!(h.status(&flow).await, "human");
    assert_eq!(held(&h).await, (1, 1));
    assert_eq!(
        finishing(&services, &flow, None).await.unwrap_err().code(),
        antibot::CAPTCHA_REQUIRED
    );
    h.back(Provider::Google, &flow, None).await;
    assert_eq!(h.status(&flow).await, "human");
    let site = services.clone();
    let browser = flow.start.browser.clone();
    let solved = tokio::spawn(async move {
        site.oauth()
            .finish_with(Some(&browser), &meta(), Some("slow:ok:signup"))
            .await
    });
    tokio::time::sleep(SLOW / 4).await;
    assert!(!solved.is_finished(), "капча ответила слишком рано");
    base_is_free(&store).await;
    assert!(confirmed(solved.await.unwrap()).created);
    assert_eq!(held(&h).await, (0, 0));
    assert_eq!(h.exchanges(Provider::Google, &flow).await, 1);
    assert_eq!(h.clients_count().await, 1);
    assert_eq!(h.sessions_count().await, 1);
    store.close().await;
}

#[tokio::test]
async fn язык_нового_клиента_берётся_из_запроса_возврата() {
    let Some(h) = harness().await else { return };
    h.services
        .settings_ops()
        .update_languages(
            &Languages {
                english: true,
                default: Locale::Ru,
            },
            h.admin,
        )
        .await
        .unwrap();
    let flow = h.login_flow(Provider::Google).await;
    h.slow(Provider::Google, &flow, "9961", Delay::ZERO).await;
    lang::scope(Locale::En, returning(&h.services, Provider::Google, &flow))
        .await
        .unwrap();
    let english = confirmed(h.finish(&flow).await).principal.client_id;
    let plain = confirmed(h.sign_in(Provider::Google, "9962", person(62), None).await)
        .principal
        .client_id;
    let mut conn = h.services.store().conn().await.unwrap();
    for (client, locale) in [(english, Locale::En), (plain, Locale::Ru)] {
        assert_eq!(conn.clients().get(client).await.unwrap().locale, locale);
    }
}
