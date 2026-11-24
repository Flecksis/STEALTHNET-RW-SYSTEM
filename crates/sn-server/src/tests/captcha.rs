use super::*;
use sn_domain::module::ModuleId;
use sn_modules::ModuleConfig;
use sn_modules::catalog::antibot;
use sn_modules::catalog::antibot::captcha::Endpoints;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SECRET: &str = "0x4AAAAAAAsecret-key";

async fn trial_on(h: &Http) {
    {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.settings()
            .set(
                keys::TRIAL,
                &serde_json::to_value(sn_domain::settings::TrialSettings::single(
                    true,
                    3,
                    vec![SquadId(Uuid::from_u128(1))],
                    sn_domain::traffic::TrafficPolicy::UNLIMITED,
                    1,
                ))
                .unwrap(),
                None,
            )
            .await
            .unwrap();
    }
    h.services().reload().await.unwrap();
    h.services().set_remna(Some(h.remna.clone()));
}

async fn antibot_on(h: &Http, config: Value) {
    let modules = h.services().modules();
    let store = h.services().store();
    modules
        .set_config(
            store,
            ModuleId::Antibot,
            ModuleConfig::from_value(&config),
            None,
        )
        .await
        .unwrap();
    modules
        .set_enabled(store, ModuleId::Antibot, true, None)
        .await
        .unwrap();
}

fn form_value(body: &[u8], key: &str) -> String {
    String::from_utf8_lossy(body)
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")).map(str::to_string))
        .unwrap_or_default()
        .replace("%3A", ":")
}

async fn siteverify(h: &Http) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/turnstile"))
        .respond_with(|request: &wiremock::Request| {
            let token = form_value(&request.body, "response");
            match token.strip_prefix("ok:") {
                Some(rest) => {
                    let (action, cdata) = rest.split_once(':').unwrap_or((rest, ""));
                    let mut body = json!({ "success": true, "action": action });
                    if !cdata.is_empty() {
                        body["cdata"] = json!(cdata);
                    }
                    ResponseTemplate::new(200).set_body_json(body)
                }
                None => ResponseTemplate::new(200).set_body_json(json!({
                    "success": false,
                    "error-codes": ["invalid-input-response"],
                })),
            }
        })
        .mount(&server)
        .await;
    h.services().set_captcha_endpoints(Endpoints {
        turnstile: format!("{}/turnstile", server.uri()),
        yandex: format!("{}/yandex", server.uri()),
    });
    server
}

async fn age_checks(h: &Http, client: ClientId) {
    let mut conn = h.services().store().conn().await.unwrap();
    sqlx::query(
        "UPDATE antibot_checks SET created_at = now() - interval '5 seconds', bits = LEAST(bits, 4) WHERE client_id = $1",
    )
    .bind(client.0)
    .execute(conn.conn())
    .await
    .unwrap();
}

#[tokio::test]
async fn без_внешней_капчи_эхо_токена_больше_не_даёт_пробник() {
    let Some(h) = http().await else { return };
    trial_on(&h).await;
    antibot_on(
        &h,
        json!({ "mode": "confirm", "burst_count": 0, "check_username": false }),
    )
    .await;
    let client = h.client(5_100_000_001).await;
    {
        let mut conn = h.services().store().conn().await.unwrap();
        antibot::store::save_mark(
            &mut conn,
            client,
            &[antibot::signals::Signal::DisposableEmail],
            &json!({ "domain": "mailinator.com" }),
        )
        .await
        .unwrap();
    }
    let token = h.client_token(client).await;
    let (status, first) = h
        .write(Method::POST, "/api/client/trial", &token, json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["again"], "ask");
    assert_eq!(first["confirm"]["check"]["kind"], "work");
    assert_eq!(first["confirm"]["check"]["bits"], 18);
    let echo = first["confirm"]["token"].as_str().unwrap().to_string();
    age_checks(&h, client).await;
    let (status, second) = h
        .write(
            Method::POST,
            "/api/client/trial",
            &token,
            json!({ "confirm": echo }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["again"], "failed");
    assert!(second.get("order_id").is_none());
    let fresh = second["confirm"]["token"].as_str().unwrap().to_string();
    assert_ne!(fresh, echo);
    age_checks(&h, client).await;
    let proof = antibot::checks::solve(&fresh, 4);
    let (status, done) = h
        .write(
            Method::POST,
            "/api/client/trial",
            &token,
            json!({ "confirm": fresh, "proof": proof }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert!(done["order_id"].is_i64(), "{done}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn капча_перед_пробником_в_кабинете_и_на_странице_проверки() {
    let Some(h) = http().await else { return };
    trial_on(&h).await;
    h.set_bot().await;
    antibot_on(
        &h,
        json!({
            "mode": "confirm",
            "burst_count": 0,
            "check_username": false,
            "captcha": "turnstile",
            "captcha_site_key": "0x4AAAAAAAsite-key",
            "captcha_secret": SECRET,
        }),
    )
    .await;
    let _stub = siteverify(&h).await;
    let cabinet = h.client(5_200_000_001).await;
    let token = h.client_token(cabinet).await;
    let (status, asked) = h
        .write(Method::POST, "/api/client/trial", &token, json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{asked}");
    let check = &asked["confirm"]["check"];
    assert_eq!(check["kind"], "captcha");
    assert_eq!(check["provider"], "turnstile");
    assert_eq!(check["site_key"], "0x4AAAAAAAsite-key");
    assert_eq!(check["action"], "trial");
    assert!(!asked.to_string().contains(SECRET));
    let first = asked["confirm"]["token"].as_str().unwrap().to_string();
    let (status, failed) = h
        .write(
            Method::POST,
            "/api/client/trial",
            &token,
            json!({ "confirm": first, "captcha": "bad" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{failed}");
    assert_eq!(failed["again"], "failed");
    let next = failed["confirm"]["token"].as_str().unwrap().to_string();
    let cdata = failed["confirm"]["check"]["cdata"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, done) = h
        .write(
            Method::POST,
            "/api/client/trial",
            &token,
            json!({ "confirm": next, "captcha": format!("ok:trial:{cdata}") }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert!(done["order_id"].is_i64(), "{done}");

    let bot_client = h.client(5_300_000_001).await;
    let challenge = h
        .services()
        .checkout()
        .trial_challenge(bot_client, antibot::Surface::Bot, None)
        .await
        .unwrap();
    assert!(challenge.captcha());
    let (status, _, missing) = h
        .send(
            Method::POST,
            "/api/public/verify",
            None,
            Some(json!({ "token": "0123456789abcdef0123456789abcdef" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{missing}");
    assert_eq!(missing["error"]["code"], antibot::EXPIRED);
    let (status, _, view) = h
        .send(
            Method::POST,
            "/api/public/verify",
            None,
            Some(json!({ "token": challenge.token })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["purpose"], "trial");
    assert_eq!(view["captcha"]["provider"], "turnstile");
    assert_eq!(view["back"]["bot"], "https://t.me/stealth_vpn_bot");
    assert_eq!(view["locale"], "ru");
    assert!(!view.to_string().contains(SECRET));
    let cdata = view["captcha"]["cdata"].as_str().unwrap().to_string();
    let (status, _, passed) = h
        .send(
            Method::POST,
            "/api/public/verify/pass",
            None,
            Some(json!({ "token": challenge.token, "captcha": format!("ok:trial:{cdata}") })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{passed}");
    assert_eq!(passed["done"], true);
    assert_eq!(
        h.services()
            .checkout()
            .start_trial(bot_client, None)
            .await
            .unwrap_err()
            .code(),
        "trial.used"
    );
    let (status, _, again) = h
        .send(
            Method::POST,
            "/api/public/verify/pass",
            None,
            Some(json!({ "token": challenge.token, "captcha": format!("ok:trial:{cdata}") })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{again}");
    assert_eq!(again["error"]["code"], antibot::EXPIRED);
    h.db.cleanup().await;
}

#[tokio::test]
async fn сайт_отдаёт_ключ_капчи_для_регистрации_без_секрета() {
    let Some(h) = http().await else { return };
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert!(site["captcha"].is_null(), "{site}");
    antibot_on(
        &h,
        json!({
            "captcha": "yandex",
            "captcha_site_key": "ysc1_site-key",
            "captcha_secret": SECRET,
        }),
    )
    .await;
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["captcha"]["provider"], "yandex");
    assert_eq!(site["captcha"]["site_key"], "ysc1_site-key");
    assert!(!site.to_string().contains(SECRET));
    antibot_on(
        &h,
        json!({
            "captcha": "yandex",
            "captcha_site_key": "ysc1_site-key",
            "captcha_secret": SECRET,
            "captcha_signup": false,
        }),
    )
    .await;
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert!(site["captcha"].is_null(), "{site}");
    h.db.cleanup().await;
}
