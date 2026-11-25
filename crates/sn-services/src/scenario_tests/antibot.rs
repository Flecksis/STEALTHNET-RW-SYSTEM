use super::siteverify::{SLOW, siteverify};
use crate::auth::RequestMeta;
use crate::checkout::testkit::{Kit, client, kit_with, set_trial_variants, trial_variant};
use crate::services::{EnvFallbacks, ServiceDeps};
use crate::signup::Arrival;
use crate::{Services, SystemClock};
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_domain::ids::ClientId;
use sn_domain::module::ModuleId;
use sn_domain::paging::PageRequest;
use sn_modules::catalog::antibot::captcha::{Provider, Widget};
use sn_modules::catalog::antibot::{
    self, Answer, AntibotModule, Check, JournalFilter, Surface, checks,
};
use sn_modules::catalog::mail::MailModule;
use sn_modules::{Module, ModuleConfig, ModuleRegistry};
use sn_payments::ProviderRegistry;
use sn_store::testing::{DAY, wait_for_window_room};
use sn_store::{Db, Store, StoreOptions};
use std::sync::Arc;
use std::time::{Duration, Instant};

async fn antibot_kit() -> Option<Kit> {
    let kit = kit_with(vec![Arc::new(AntibotModule)]).await?;
    for id in 1..=25 {
        client(&kit.services, 1_000 + id).await;
    }
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query(
        "UPDATE client_identities SET created_at = now() - interval '90 days' WHERE kind = 'telegram'",
    )
    .execute(conn.conn())
    .await
    .unwrap();
    drop(conn);
    Some(kit)
}

async fn fresh(services: &Services, id: i64, username: Option<&str>) -> ClientId {
    services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id,
                username: username.map(str::to_string),
                first_name: Some("Новый".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap()
        .0
        .id
}

async fn mode(services: &Services, mode: &str) {
    services
        .modules()
        .set_config(
            services.store(),
            ModuleId::Antibot,
            ModuleConfig::from_value(&serde_json::json!({ "mode": mode })),
            None,
        )
        .await
        .unwrap();
}

async fn actions(services: &Services, client_id: ClientId) -> Vec<&'static str> {
    let mut conn = services.store().conn().await.unwrap();
    let mut page = antibot::store::journal(
        &mut conn,
        JournalFilter::All,
        Some(client_id),
        PageRequest::default(),
    )
    .await
    .unwrap();
    page.items.reverse();
    page.items.into_iter().map(|i| i.action.as_str()).collect()
}

async fn age_challenge(services: &Services, client_id: ClientId) {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(
        "UPDATE antibot_checks SET created_at = now() - interval '5 seconds', bits = LEAST(bits, 4) WHERE client_id = $1",
    )
    .bind(client_id.0)
    .execute(conn.conn())
    .await
    .unwrap();
}

#[tokio::test]
async fn после_проверки_антибота_выдаётся_выбранный_вариант() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    set_trial_variants(
        &kit,
        vec![
            trial_variant(1, "Россия", true, 3, 1, &[1]),
            trial_variant(2, "Европа", true, 1, 2, &[2]),
        ],
    )
    .await;
    let services = &kit.services;
    let checkout = services.checkout();
    let suspect = fresh(services, 8_600_000_000, None).await;
    assert_eq!(
        checkout
            .start_trial(suspect, None)
            .await
            .unwrap_err()
            .code(),
        "trial.variant_required"
    );
    assert_eq!(
        checkout
            .start_trial(suspect, Some(2))
            .await
            .unwrap_err()
            .code(),
        antibot::CONFIRM_CODE
    );
    let token = checkout
        .trial_challenge(suspect, Surface::Bot, Some(2))
        .await
        .unwrap()
        .token;
    age_challenge(services, suspect).await;
    let order_id = checkout
        .confirm_trial(suspect, &token, Answer::button(), Some(2))
        .await
        .unwrap();
    let order = crate::checkout::testkit::order(services, order_id).await;
    let snapshot = order.snapshot.unwrap();
    assert_eq!((snapshot.name.as_str(), snapshot.days), ("Европа", 1));
    assert_eq!(
        checkout
            .start_trial(suspect, Some(1))
            .await
            .unwrap_err()
            .code(),
        "trial.used"
    );
    assert_eq!(
        actions(services, suspect).await,
        ["marked", "asked", "confirmed"]
    );
    mode(services, "block").await;
    let denied = fresh(services, 8_700_000_000, None).await;
    assert_eq!(
        checkout
            .start_trial(denied, Some(1))
            .await
            .unwrap_err()
            .code(),
        antibot::DENIED_CODE
    );
    assert_eq!(actions(services, denied).await, ["marked", "trial_denied"]);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn новый_аккаунт_без_username_проходит_проверку_и_получает_пробник() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    let checkout = services.checkout();
    let calm = fresh(services, 1_010_000_000, Some("real_user")).await;
    checkout.start_trial(calm, None).await.unwrap();
    assert!(actions(services, calm).await.is_empty());
    let suspect = fresh(services, 8_100_000_000, None).await;
    let err = checkout.start_trial(suspect, None).await.unwrap_err();
    assert_eq!(err.code(), antibot::CONFIRM_CODE);
    let challenge = checkout
        .trial_challenge(suspect, Surface::Web, None)
        .await
        .unwrap();
    assert_eq!(
        challenge.check,
        Check::Work {
            bits: checks::WORK_BITS
        }
    );
    let echo = Answer::default();
    assert_eq!(
        checkout
            .confirm_trial(suspect, &challenge.token, echo, None)
            .await
            .unwrap_err()
            .code(),
        antibot::TOO_FAST
    );
    age_challenge(services, suspect).await;
    assert_eq!(
        checkout
            .confirm_trial(suspect, &challenge.token, echo, None)
            .await
            .unwrap_err()
            .code(),
        antibot::FAILED
    );
    let late = checks::solve(&challenge.token, 4);
    assert_eq!(
        checkout
            .confirm_trial(
                suspect,
                &challenge.token,
                Answer::web(Some(&late), None, None),
                None,
            )
            .await
            .unwrap_err()
            .code(),
        antibot::EXPIRED
    );
    let challenge = checkout
        .trial_challenge(suspect, Surface::Web, None)
        .await
        .unwrap();
    age_challenge(services, suspect).await;
    let proof = checks::solve(&challenge.token, 4);
    assert_eq!(
        checkout
            .confirm_trial(
                suspect,
                "0123456789abcdef0123456789abcdef",
                Answer::web(Some(&proof), None, None),
                None,
            )
            .await
            .unwrap_err()
            .code(),
        antibot::EXPIRED
    );
    assert_eq!(
        checkout
            .confirm_trial(
                calm,
                &challenge.token,
                Answer::web(Some(&proof), None, None),
                None,
            )
            .await
            .unwrap_err()
            .code(),
        antibot::EXPIRED
    );
    checkout
        .confirm_trial(
            suspect,
            &challenge.token,
            Answer::web(Some(&proof), None, None),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        checkout
            .start_trial(suspect, None)
            .await
            .unwrap_err()
            .code(),
        "trial.used"
    );
    assert_eq!(
        actions(services, suspect).await,
        ["marked", "asked", "confirmed"]
    );
    let mut conn = services.store().conn().await.unwrap();
    let mark = antibot::store::mark_of(&mut conn, suspect)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        mark.reasons,
        [
            antibot::signals::Signal::YoungAccount,
            antibot::signals::Signal::NoUsername
        ]
    );
    assert!(mark.confirmed_at.is_some());
    drop(conn);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn в_режиме_запрета_пробника_нет_пока_админ_не_снимет_отметку() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    wait_for_window_room(kit.services.store(), DAY).await;
    let services = &kit.services;
    mode(services, "block").await;
    let checkout = services.checkout();
    let suspect = fresh(services, 8_200_000_000, None).await;
    assert_eq!(
        checkout
            .start_trial(suspect, None)
            .await
            .unwrap_err()
            .code(),
        antibot::DENIED_CODE
    );
    assert_eq!(
        checkout
            .start_trial(suspect, None)
            .await
            .unwrap_err()
            .code(),
        antibot::DENIED_CODE
    );
    assert_eq!(actions(services, suspect).await, ["marked", "trial_denied"]);
    let ctx = services.module_context();
    let admin = crate::scenario_tests::owner(services).await;
    assert!(antibot::clear(&ctx, suspect, Some(admin)).await.unwrap());
    assert!(!antibot::clear(&ctx, suspect, Some(admin)).await.unwrap());
    let mut conn = services.store().conn().await.unwrap();
    let logged: Vec<(String, i64, i64, serde_json::Value)> = sqlx::query_as(
        "SELECT actor_kind, actor_id, entity_id, details FROM audit_log \
         WHERE action = 'client.antibot_cleared' AND entity_kind = 'client'",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap();
    drop(conn);
    assert_eq!(
        logged,
        [(
            "admin".to_string(),
            admin.0,
            suspect.0,
            serde_json::json!({ "reason": "новый аккаунт Telegram, нет @username" })
        )]
    );
    checkout.start_trial(suspect, None).await.unwrap();
    assert_eq!(
        actions(services, suspect).await,
        ["marked", "trial_denied", "cleared"]
    );
    mode(services, "mark").await;
    let quiet = fresh(services, 8_300_000_000, None).await;
    checkout.start_trial(quiet, None).await.unwrap();
    assert_eq!(actions(services, quiet).await, ["marked"]);
    services
        .modules()
        .set_enabled(services.store(), ModuleId::Antibot, false, None)
        .await
        .unwrap();
    let off = fresh(services, 8_400_000_000, None).await;
    checkout.start_trial(off, None).await.unwrap();
    assert!(actions(services, off).await.is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn смена_почты_не_даёт_второй_пробный_период_тому_же_клиенту() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    let checkout = services.checkout();
    let mut conn = services.store().conn().await.unwrap();
    let id = conn
        .clients()
        .create(&sn_store::NewClient {
            locale: sn_domain::client::Locale::Ru,
            registration_source: RegistrationSource::Web,
            note: None,
        })
        .await
        .unwrap()
        .id;
    let email = |value: &str| {
        sn_domain::client::NewIdentity::new(sn_domain::client::IdentityKind::Email, value, true)
            .unwrap()
    };
    conn.clients()
        .add_identity(id, &email("first@example.org"))
        .await
        .unwrap();
    drop(conn);
    assert!(checkout.trial_open(id).await.unwrap());
    checkout.start_trial(id, None).await.unwrap();
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query("DELETE FROM client_identities WHERE client_id = $1")
        .bind(id.0)
        .execute(conn.conn())
        .await
        .unwrap();
    conn.clients()
        .add_identity(id, &email("second@example.org"))
        .await
        .unwrap();
    drop(conn);
    assert!(!checkout.trial_open(id).await.unwrap());
    assert_eq!(
        checkout.start_trial(id, None).await.unwrap_err().code(),
        "trial.used"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn почта_из_одноразового_домена_и_вариант_чужого_адреса_отмечаются() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    let ctx = services.module_context();
    let mut conn = services.store().conn().await.unwrap();
    let mut clients = Vec::new();
    for email in [
        "ivan.petrov@gmail.com",
        "ivanpetrov+vpn@googlemail.com",
        "bot@mailinator.com",
        "anna@example.org",
    ] {
        let id = conn
            .clients()
            .create(&sn_store::NewClient {
                locale: sn_domain::client::Locale::Ru,
                registration_source: RegistrationSource::Web,
                note: None,
            })
            .await
            .unwrap()
            .id;
        conn.clients()
            .add_identity(
                id,
                &sn_domain::client::NewIdentity::new(
                    sn_domain::client::IdentityKind::Email,
                    email,
                    true,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        clients.push(id);
    }
    let settings = antibot::AntibotSettings::default();
    let now = sn_modules::catalog::db_now(&mut conn).await.unwrap();
    let first = antibot::assess(&mut conn, clients[0], &settings, now)
        .await
        .unwrap();
    assert!(first.signals.is_empty(), "{:?}", first.signals);
    let alias = antibot::assess(&mut conn, clients[1], &settings, now)
        .await
        .unwrap();
    assert!(alias.suspicious());
    assert_eq!(alias.signals, [antibot::signals::Signal::EmailAlias]);
    assert_eq!(alias.details["alias_of"], serde_json::json!(clients[0].0));
    let disposable = antibot::assess(&mut conn, clients[2], &settings, now)
        .await
        .unwrap();
    assert_eq!(
        disposable.signals,
        [antibot::signals::Signal::DisposableEmail]
    );
    let clean = antibot::assess(&mut conn, clients[3], &settings, now)
        .await
        .unwrap();
    assert!(clean.signals.is_empty());
    drop(conn);
    assert_eq!(
        antibot::trial_gate(&ctx, clients[2]).await.unwrap(),
        antibot::Gate::Confirm
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn регистрации_в_кабинете_с_одного_адреса_сверх_лимита_отмечаются() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    let mut conn = services.store().conn().await.unwrap();
    let mut clients = Vec::new();
    for n in 0..5 {
        let id = conn
            .clients()
            .create(&sn_store::NewClient {
                locale: sn_domain::client::Locale::Ru,
                registration_source: RegistrationSource::Web,
                note: None,
            })
            .await
            .unwrap()
            .id;
        let ip = if n == 4 {
            "198.51.100.20"
        } else {
            "203.0.113.7"
        };
        sqlx::query(
            "INSERT INTO client_sessions (client_id, token_hash, channel, ip, expires_at) \
             VALUES ($1, $2, 'cabinet', $3, now() + interval '1 day')",
        )
        .bind(id.0)
        .bind(format!("token-{}", id.0).into_bytes())
        .bind(ip)
        .execute(conn.conn())
        .await
        .unwrap();
        clients.push(id);
    }
    let settings = antibot::AntibotSettings::default();
    let now = sn_modules::catalog::db_now(&mut conn).await.unwrap();
    for (n, id) in clients.iter().enumerate() {
        let found = antibot::assess(&mut conn, *id, &settings, now)
            .await
            .unwrap();
        let limited = found.signals.contains(&antibot::signals::Signal::IpLimit);
        assert_eq!(limited, n == 3, "клиент {n}: {:?}", found.signals);
    }
    drop(conn);
    kit.db.cleanup().await;
}

async fn captcha_on(services: &Services, provider: &str, trial: &str) {
    let secret = if provider == "yandex" {
        "ysc2_secret-key"
    } else {
        "0x4AAAAAAAsecret-key"
    };
    captcha_with(services, provider, trial, secret).await;
}

async fn captcha_with(services: &Services, provider: &str, trial: &str, secret: &str) {
    services
        .modules()
        .set_config(
            services.store(),
            ModuleId::Antibot,
            ModuleConfig::from_value(&serde_json::json!({
                "mode": "confirm",
                "captcha": provider,
                "captcha_site_key": if provider == "yandex" { "ysc1_site-key" } else { "0x4AAAAAAAsite-key" },
                "captcha_secret": secret,
                "captcha_trial": trial,
            })),
            None,
        )
        .await
        .unwrap();
}

fn widget(challenge: &antibot::Challenge) -> Widget {
    match &challenge.check {
        Check::Captcha(widget) => widget.clone(),
        other => panic!("ожидалась капча, а пришло {other:?}"),
    }
}

const IP: Option<&str> = Some("203.0.113.7");

fn captcha(token: &str) -> Answer<'_> {
    Answer::web(None, Some(token), IP)
}

async fn trial_code(
    services: &Services,
    client: ClientId,
    token: &str,
    answer: Answer<'_>,
) -> String {
    services
        .checkout()
        .confirm_trial(client, token, answer, None)
        .await
        .unwrap_err()
        .code()
        .to_string()
}

#[tokio::test]
async fn капча_перед_пробником_проверяется_на_сервере_один_раз_и_только_для_своего_клиента() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    let stub = siteverify(services).await;
    captcha_on(services, "turnstile", "all").await;
    let checkout = services.checkout();
    let calm = fresh(services, 1_020_000_000, Some("calm_user")).await;
    let other = fresh(services, 1_045_000_000, Some("other_user")).await;
    assert_eq!(
        checkout.start_trial(calm, None).await.unwrap_err().code(),
        antibot::CONFIRM_CODE
    );
    let challenge = checkout
        .trial_challenge(calm, Surface::Web, None)
        .await
        .unwrap();
    let shown = widget(&challenge);
    assert_eq!(shown.provider, Provider::Turnstile);
    assert_eq!(shown.site_key, "0x4AAAAAAAsite-key");
    assert_eq!(shown.action, "trial");
    let cdata = shown.cdata.clone().unwrap();
    assert_eq!(cdata.len(), 32);
    let token = challenge.token.as_str();
    assert_eq!(
        trial_code(services, calm, token, Answer::web(None, None, IP)).await,
        antibot::FAILED
    );
    assert_eq!(
        trial_code(services, calm, token, captcha("bad-1")).await,
        antibot::FAILED
    );
    let signup = format!("ok:signup:{cdata}");
    assert_eq!(
        trial_code(services, calm, token, captcha(&signup)).await,
        antibot::FAILED
    );
    assert_eq!(
        trial_code(services, calm, token, captcha("ok:trial:0000")).await,
        antibot::FAILED
    );
    assert_eq!(
        trial_code(services, calm, token, captcha("down-1")).await,
        antibot::UNAVAILABLE
    );
    assert_eq!(
        trial_code(services, calm, token, captcha("secret-1")).await,
        antibot::UNAVAILABLE
    );
    assert_eq!(
        checkout.start_trial(calm, None).await.unwrap_err().code(),
        antibot::CONFIRM_CODE
    );
    let good = format!("ok:trial:{cdata}");
    checkout
        .confirm_trial(calm, token, captcha(&good), None)
        .await
        .unwrap();
    let sent = stub.last();
    assert_eq!(sent["secret"], "0x4AAAAAAAsecret-key");
    assert_eq!(sent["response"], good);
    assert_eq!(sent["remoteip"], "203.0.113.7");
    assert_eq!(sent["idempotency_key"].len(), 36);
    assert_eq!(
        checkout.start_trial(calm, None).await.unwrap_err().code(),
        "trial.used"
    );
    let theirs = checkout
        .trial_challenge(other, Surface::Web, None)
        .await
        .unwrap();
    let asked = stub.calls();
    assert_eq!(
        trial_code(services, other, &theirs.token, captcha(&good)).await,
        antibot::FAILED
    );
    assert_eq!(stub.calls(), asked);
    let their_cdata = widget(&theirs).cdata.unwrap();
    assert_ne!(their_cdata, cdata);
    let theirs_good = format!("ok:trial:{their_cdata}");
    assert_eq!(
        trial_code(services, calm, &theirs.token, captcha(&theirs_good)).await,
        antibot::EXPIRED
    );
    for n in 0..4 {
        assert_eq!(
            trial_code(services, other, &theirs.token, captcha(&format!("bad-{n}"))).await,
            antibot::FAILED
        );
    }
    assert_eq!(
        trial_code(
            services,
            other,
            &theirs.token,
            captcha(&format!("ok:trial:{their_cdata}:late"))
        )
        .await,
        antibot::EXPIRED
    );
    assert!(actions(services, calm).await.is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ответ_turnstile_без_привязки_к_действию_и_клиенту_не_принимается() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    let _stub = siteverify(services).await;
    captcha_on(services, "turnstile", "all").await;
    let checkout = services.checkout();
    let client = fresh(services, 1_020_000_000, Some("calm_user")).await;
    let challenge = checkout
        .trial_challenge(client, Surface::Web, None)
        .await
        .unwrap();
    let cdata = widget(&challenge).cdata.unwrap();
    for unbound in ["bare-1", "ok:trial:", &format!("ok::{cdata}")] {
        assert_eq!(
            trial_code(services, client, &challenge.token, captcha(unbound)).await,
            antibot::FAILED,
            "{unbound}"
        );
    }
    let tester = fresh(services, 1_045_000_000, Some("tester")).await;
    captcha_with(
        services,
        "turnstile",
        "all",
        "1x0000000000000000000000000000000AA",
    )
    .await;
    let challenge = checkout
        .trial_challenge(tester, Surface::Web, None)
        .await
        .unwrap();
    assert_eq!(
        trial_code(services, tester, &challenge.token, captcha("ok:signup:")).await,
        antibot::FAILED
    );
    checkout
        .confirm_trial(tester, &challenge.token, captcha("bare-2"), None)
        .await
        .unwrap();
    kit.db.cleanup().await;
}

#[tokio::test]
async fn сбой_сервиса_капчи_виден_другой_копии_панели_и_гаснет_после_удачной_проверки() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    let _stub = siteverify(services).await;
    captcha_on(services, "turnstile", "all").await;
    let client = fresh(services, 1_020_000_000, Some("calm_user")).await;
    let challenge = services
        .checkout()
        .trial_challenge(client, Surface::Web, None)
        .await
        .unwrap();
    let cdata = widget(&challenge).cdata.unwrap();
    let (store, _copy) = tight(&kit, vec![Arc::new(AntibotModule)]).await;
    let seen = async || {
        let mut conn = store.conn().await.unwrap();
        antibot::captcha::health(&mut conn)
            .await
            .unwrap()
            .map(|health| (health.provider, health.problem))
    };
    assert_eq!(seen().await, None);
    assert_eq!(
        trial_code(services, client, &challenge.token, captcha("secret-1")).await,
        antibot::UNAVAILABLE
    );
    assert_eq!(
        seen().await,
        Some((Provider::Turnstile, "секретный ключ не подошёл".to_string()))
    );
    services
        .checkout()
        .confirm_trial(
            client,
            &challenge.token,
            captcha(&format!("ok:trial:{cdata}")),
            None,
        )
        .await
        .unwrap();
    assert_eq!(seen().await, None);
    store.close().await;
    kit.db.cleanup().await;
}

#[tokio::test]
async fn капча_только_подозрительным_заменяет_кнопку_и_не_засчитывает_прежнее_подтверждение() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    let stub = siteverify(services).await;
    captcha_on(services, "yandex", "suspicious").await;
    let checkout = services.checkout();
    let ctx = services.module_context();
    let calm = fresh(services, 1_060_000_000, Some("plain_user")).await;
    checkout.start_trial(calm, None).await.unwrap();
    assert_eq!(stub.calls(), 0);
    let suspect = fresh(services, 8_300_000_000, None).await;
    assert_eq!(
        checkout
            .start_trial(suspect, None)
            .await
            .unwrap_err()
            .code(),
        antibot::CONFIRM_CODE
    );
    let button = antibot::challenge(&ctx, suspect, "roulette", Surface::Bot)
        .await
        .unwrap();
    assert_eq!(button.check, Check::Button);
    age_challenge(services, suspect).await;
    assert_eq!(
        antibot::confirm(&ctx, suspect, &button.token, "roulette", Answer::button())
            .await
            .unwrap(),
        antibot::Confirmation::Confirmed
    );
    assert_eq!(
        checkout
            .start_trial(suspect, None)
            .await
            .unwrap_err()
            .code(),
        antibot::CONFIRM_CODE
    );
    let challenge = checkout
        .trial_challenge(suspect, Surface::Bot, None)
        .await
        .unwrap();
    let shown = widget(&challenge);
    assert_eq!(shown.provider, Provider::Yandex);
    let token = challenge.token.as_str();
    assert_eq!(
        trial_code(services, suspect, token, captcha("nohost-1")).await,
        antibot::UNAVAILABLE
    );
    assert_eq!(
        trial_code(services, suspect, token, captcha("down-1")).await,
        antibot::UNAVAILABLE
    );
    assert_eq!(
        trial_code(services, suspect, token, captcha("bad-1")).await,
        antibot::FAILED
    );
    checkout
        .confirm_trial(suspect, token, captcha("ok-1"), None)
        .await
        .unwrap();
    let sent = stub.last();
    assert_eq!(sent["secret"], "ysc2_secret-key");
    assert_eq!(sent["token"], "ok-1");
    assert_eq!(sent["ip"], "203.0.113.7");
    assert_eq!(
        actions(services, suspect).await,
        ["marked", "asked", "confirmed"]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn в_режиме_запрета_капча_не_спрашивается_а_без_модуля_её_нет() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    let stub = siteverify(services).await;
    services
        .modules()
        .set_config(
            services.store(),
            ModuleId::Antibot,
            ModuleConfig::from_value(&serde_json::json!({
                "mode": "block",
                "captcha": "turnstile",
                "captcha_site_key": "0x4AAAAAAAsite-key",
                "captcha_secret": "0x4AAAAAAAsecret-key",
            })),
            None,
        )
        .await
        .unwrap();
    let checkout = services.checkout();
    let suspect = fresh(services, 8_400_000_000, None).await;
    assert_eq!(
        checkout
            .start_trial(suspect, None)
            .await
            .unwrap_err()
            .code(),
        antibot::DENIED_CODE
    );
    let calm = fresh(services, 1_070_000_000, Some("calm_again")).await;
    assert_eq!(
        checkout.start_trial(calm, None).await.unwrap_err().code(),
        antibot::CONFIRM_CODE
    );
    services
        .modules()
        .set_enabled(services.store(), ModuleId::Antibot, false, None)
        .await
        .unwrap();
    checkout.start_trial(calm, None).await.unwrap();
    assert_eq!(stub.calls(), 0);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn страница_проверки_из_бота_выдаёт_выбранный_вариант_пробника() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    set_trial_variants(
        &kit,
        vec![
            trial_variant(1, "Россия", true, 3, 1, &[1]),
            trial_variant(2, "Европа", true, 1, 2, &[2]),
        ],
    )
    .await;
    let services = &kit.services;
    let _stub = siteverify(services).await;
    captcha_on(services, "turnstile", "all").await;
    let checkout = services.checkout();
    let ctx = services.module_context();
    let calm = fresh(services, 1_080_000_000, Some("picky_user")).await;
    assert_eq!(
        checkout
            .start_trial(calm, Some(2))
            .await
            .unwrap_err()
            .code(),
        antibot::CONFIRM_CODE
    );
    let challenge = checkout
        .trial_challenge(calm, Surface::Bot, Some(2))
        .await
        .unwrap();
    let page = checks::page_check(&ctx, &challenge.token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.client_id, calm);
    assert_eq!(page.variant, Some(2));
    let good = format!("ok:trial:{}", page.widget.cdata.clone().unwrap());
    let order_id = checkout
        .confirm_trial(
            page.client_id,
            &challenge.token,
            captcha(&good),
            page.variant,
        )
        .await
        .unwrap();
    let order = crate::checkout::testkit::order(services, order_id).await;
    let snapshot = order.snapshot.unwrap();
    assert_eq!((snapshot.name.as_str(), snapshot.days), ("Европа", 1));
    assert!(
        checks::page_check(&ctx, &challenge.token)
            .await
            .unwrap()
            .is_none()
    );
    kit.db.cleanup().await;
}

async fn tight(kit: &Kit, modules: Vec<Arc<dyn Module>>) -> (Store, Services) {
    let schema: String = {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query_scalar("SELECT current_schema()")
            .fetch_one(conn.conn())
            .await
            .unwrap()
    };
    let store = Store::connect(
        &sn_store::testing::test_database_url().unwrap(),
        StoreOptions {
            max_connections: 1,
            acquire_timeout: Duration::from_secs(2),
            search_path: Some(schema),
            ..StoreOptions::default()
        },
    )
    .await
    .unwrap();
    let services = Services::new(ServiceDeps {
        store: store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::new(modules).unwrap()),
        secrets: kit.services.secrets().cloned(),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    services.set_remna(Some(kit.remna.clone()));
    (store, services)
}

async fn free_while<T: Send + 'static>(
    store: &Store,
    work: impl Future<Output = T> + Send + 'static,
) -> T {
    let running = tokio::spawn(work);
    tokio::time::sleep(SLOW / 4).await;
    assert!(
        !running.is_finished(),
        "сервис проверки ответил слишком рано"
    );
    let started = Instant::now();
    store.ping().await.unwrap_or_else(|err| {
        panic!("пока сервис проверки отвечает, база недоступна другим запросам: {err}")
    });
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "запрос ждал соединения {:?}",
        started.elapsed()
    );
    running.await.unwrap()
}

#[tokio::test]
async fn пока_сервис_капчи_отвечает_соединение_с_базой_свободно() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    captcha_on(&kit.services, "turnstile", "all").await;
    let calm = fresh(&kit.services, 1_020_000_000, Some("calm_user")).await;
    let (store, services) = tight(&kit, vec![Arc::new(AntibotModule), Arc::new(MailModule)]).await;
    let _stub = siteverify(&services).await;
    let challenge = services
        .checkout()
        .trial_challenge(calm, Surface::Web, None)
        .await
        .unwrap();
    let cdata = widget(&challenge).cdata.unwrap();
    let shop = services.clone();
    let issued = free_while(&store, async move {
        let answer = format!("slow:ok:trial:{cdata}");
        shop.checkout()
            .confirm_trial(calm, &challenge.token, captcha(&answer), None)
            .await
    })
    .await;
    assert!(issued.is_ok(), "{issued:?}");
    let site = services.clone();
    let joined = free_while(&store, async move {
        let meta = RequestMeta {
            ip: Some("203.0.113.7".into()),
            user_agent: None,
        };
        site.auth()
            .email_signup_with(
                "calm@example.org",
                "longpassword1",
                &meta,
                Arrival::default(),
                Some("slow:ok:signup:"),
            )
            .await
    })
    .await;
    assert!(joined.is_ok(), "{joined:?}");
    store.close().await;
    kit.db.cleanup().await;
}

async fn site_bot(services: &Services) {
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set(
                sn_domain::settings::keys::BOT_USERNAME,
                &serde_json::json!("stealth_test_bot"),
                None,
            )
            .await
            .unwrap();
    }
    services
        .modules()
        .set_config(
            services.store(),
            ModuleId::Antibot,
            ModuleConfig::from_value(&serde_json::json!({
                "mode": "confirm", "burst_count": 0, "young_days": 0, "check_username": false, "ip_limit": 3
            })),
            None,
        )
        .await
        .unwrap();
}

async fn site_login(services: &Services, telegram_id: i64, ip: &str) -> ClientId {
    let auth = services.auth();
    let meta = RequestMeta {
        ip: Some(ip.into()),
        user_agent: None,
    };
    let start = auth.telegram_login_start(&meta).await.unwrap();
    let code = start.link.rsplit_once("?start=").unwrap().1.to_string();
    let id = fresh(services, telegram_id, Some("somebody")).await;
    let profile = TelegramProfile {
        id: telegram_id,
        username: Some("somebody".into()),
        first_name: Some("Новый".into()),
        last_name: None,
        language_code: Some("ru".into()),
        is_premium: false,
    };
    let confirmed = auth.confirm_telegram_login(&code, &profile).await.unwrap();
    assert_eq!(confirmed.client_id, id);
    id
}

async fn limited(services: &Services, client_id: ClientId) -> bool {
    let mut conn = services.store().conn().await.unwrap();
    antibot::store::mark_of(&mut conn, client_id)
        .await
        .unwrap()
        .is_some_and(|m| m.reasons.contains(&antibot::signals::Signal::IpLimit))
}

#[tokio::test]
async fn вход_через_telegram_с_сайта_считается_регистрацией_с_адреса() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    site_bot(services).await;
    let mut seen = Vec::new();
    for n in 0..5 {
        let id = site_login(services, 700_000_000 + n * 50_000_000, "203.0.113.9").await;
        seen.push(limited(services, id).await);
    }
    assert_eq!(seen, [false, false, false, true, true]);
    let elsewhere = site_login(services, 1_700_000_000, "203.0.113.200").await;
    assert!(!limited(services, elsewhere).await);
    let old = site_login(services, 1_001, "203.0.113.9").await;
    assert!(
        !limited(services, old).await,
        "давний клиент бота входит на сайт с того же адреса"
    );
    let bot = fresh(services, 1_800_000_000, Some("somebody")).await;
    antibot::on_registered(&services.module_context(), bot)
        .await
        .unwrap();
    assert!(!limited(services, bot).await, "клиент из бота без сайта");
    kit.db.cleanup().await;
}

#[tokio::test]
async fn одновременные_входы_через_telegram_с_сайта_не_обходят_счёт_с_адреса() {
    let Some(kit) = antibot_kit().await else {
        return;
    };
    let services = &kit.services;
    site_bot(services).await;
    let mut tasks = Vec::new();
    for n in 0..9 {
        let services = services.clone();
        tasks.push(tokio::spawn(async move {
            site_login(
                &services,
                700_000_000 + i64::from(n) * 50_000_000,
                "2001:db8:5:6::1",
            )
            .await
        }));
    }
    let mut made = Vec::new();
    for task in tasks {
        made.push(task.await.unwrap());
    }
    let ctx = services.module_context();
    let mut asked = 0;
    for id in &made {
        match antibot::trial_gate(&ctx, *id).await.unwrap() {
            antibot::Gate::Allow => {}
            antibot::Gate::Confirm => asked += 1,
            antibot::Gate::Deny => panic!("режим подтверждения не отказывает"),
        }
    }
    assert_eq!(asked, made.len() - 3);
    kit.db.cleanup().await;
}
