use super::*;
use serde_json::{Value, json};
use sn_domain::module::ModuleId;
use sn_domain::settings::{FloodSettings, keys};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

struct Limits {
    global: (f64, Instant),
    chats: std::collections::HashMap<i64, (f64, Instant)>,
}

#[derive(Clone)]
struct FakeTelegram {
    updates: Arc<Vec<Value>>,
    sent: Arc<Mutex<Vec<(i64, String, Instant)>>>,
    limits: Arc<Mutex<Limits>>,
    rejected: Arc<AtomicU64>,
    served: Arc<AtomicU64>,
}

impl FakeTelegram {
    fn new(updates: Vec<Value>) -> Self {
        let now = Instant::now();
        FakeTelegram {
            updates: Arc::new(updates),
            sent: Arc::default(),
            limits: Arc::new(Mutex::new(Limits {
                global: (30.0, now),
                chats: std::collections::HashMap::new(),
            })),
            rejected: Arc::default(),
            served: Arc::default(),
        }
    }

    fn allowed(&self, chat: i64) -> bool {
        let now = Instant::now();
        let mut limits = self.limits.lock().unwrap();
        let refill = |(tokens, at): (f64, Instant), rate: f64, cap: f64| {
            let tokens = (tokens + now.duration_since(at).as_secs_f64() * rate).min(cap);
            (tokens, now)
        };
        let global = refill(limits.global, 30.0, 31.0);
        let chat_bucket = refill(*limits.chats.get(&chat).unwrap_or(&(4.0, now)), 1.0, 4.0);
        if global.0 < 1.0 || chat_bucket.0 < 1.0 {
            limits.global = global;
            limits.chats.insert(chat, chat_bucket);
            return false;
        }
        limits.global = (global.0 - 1.0, now);
        limits.chats.insert(chat, (chat_bucket.0 - 1.0, now));
        true
    }

    fn texts_to(&self, chat: i64) -> Vec<String> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|(to, _, _)| *to == chat)
            .map(|(_, text, _)| text.clone())
            .collect()
    }

    fn first_to(&self, chat: i64) -> Option<Instant> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .find(|(to, _, _)| *to == chat)
            .map(|(_, _, at)| *at)
    }
}

impl wiremock::Respond for FakeTelegram {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let method = request.url.path().rsplit('/').next().unwrap_or_default();
        match method {
            "getUpdates" => {
                let offset = body["offset"].as_i64().unwrap_or(0);
                let batch: Vec<Value> = self
                    .updates
                    .iter()
                    .filter(|u| u["update_id"].as_i64().unwrap_or(0) >= offset)
                    .take(100)
                    .cloned()
                    .collect();
                self.served.fetch_add(batch.len() as u64, Ordering::SeqCst);
                if batch.is_empty() {
                    return ok(json!([])).set_delay(Duration::from_millis(50));
                }
                ok(json!(batch))
            }
            "sendMessage" | "editMessageText" | "sendPhoto" => {
                let chat = body["chat_id"].as_i64().unwrap_or(0);
                if !self.allowed(chat) {
                    self.rejected.fetch_add(1, Ordering::SeqCst);
                    return ResponseTemplate::new(429).set_body_json(json!({
                        "ok": false, "error_code": 429,
                        "description": "Too Many Requests: retry after 1",
                        "parameters": {"retry_after": 1}
                    }));
                }
                self.sent.lock().unwrap().push((
                    chat,
                    body["text"].as_str().unwrap_or_default().to_string(),
                    Instant::now(),
                ));
                ok(json!({"message_id": 100, "chat": {"id": chat, "type": "private"}, "date": 0}))
            }
            _ => ok(json!(true)),
        }
    }
}

async fn mount(server: &MockServer, fake: &FakeTelegram) {
    Mock::given(method("POST"))
        .and(path_regex(
            r"/(getUpdates|sendMessage|editMessageText|sendPhoto)$",
        ))
        .respond_with(fake.clone())
        .with_priority(1)
        .mount(server)
        .await;
}

fn start_from(update_id: i64, from: i64, param: Option<&str>) -> Value {
    let text = match param {
        Some(param) => format!("/start {param}"),
        None => "/start".into(),
    };
    json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id,
            "from": {"id": from, "is_bot": false, "first_name": "Фермер", "username": format!("farm{from}")},
            "chat": {"id": from, "type": "private"}, "date": 1, "text": text
        }
    })
}

fn flood_config(server: &MockServer) -> BotConfig {
    BotConfig {
        api_base: server.uri(),
        poll_timeout: Duration::from_secs(1),
        workers: 8,
        flood_report_every: Duration::from_millis(300),
        ..BotConfig::default()
    }
}

async fn count(services: &Services, sql: &str) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar(sql)
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

async fn wait_until<F, Fut>(what: &str, limit: Duration, check: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("не дождались: {what}");
}

async fn flood_settings(services: &Services, flood: FloodSettings) {
    let mut conn = services.store().conn().await.unwrap();
    conn.settings()
        .set(keys::BOT_FLOOD, &serde_json::to_value(flood).unwrap(), None)
        .await
        .unwrap();
    drop(conn);
    services.reload().await.unwrap();
}

#[tokio::test]
async fn десять_тысяч_стартов_одного_аккаунта_не_мешают_другим() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    let flooder = 7_700_001;
    let neighbour = 7_700_002;
    let mut updates: Vec<Value> = (1..=10_000)
        .map(|id| start_from(id, flooder, Some("ref_spam")))
        .collect();
    updates.insert(5_000, {
        let mut start = start_from(20_000, neighbour, None);
        start["update_id"] = json!(5_000);
        start
    });
    for (index, update) in updates.iter_mut().enumerate() {
        update["update_id"] = json!(index as i64 + 1);
    }
    let fake = FakeTelegram::new(updates);
    mount(&server, &fake).await;
    let started = Instant::now();
    let cancel = CancellationToken::new();
    let bot = tokio::spawn(Bot::new(services.clone(), flood_config(&server)).run(cancel.clone()));
    wait_until(
        "бот прочитал все обновления",
        Duration::from_secs(60),
        || async { fake.served.load(Ordering::SeqCst) >= 10_001 },
    )
    .await;
    wait_until(
        "сосед получил ответ",
        Duration::from_secs(20),
        || async { fake.first_to(neighbour).is_some() },
    )
    .await;
    wait_until(
        "блокировка записана",
        Duration::from_secs(10),
        || async {
            count(
                &services,
                "SELECT count(*) FROM audit_log WHERE action = 'settings.bot_flood_blocked'",
            )
            .await
                > 0
        },
    )
    .await;
    let elapsed = started.elapsed();
    let to_flooder = fake.texts_to(flooder);
    let neighbour_after = fake.first_to(neighbour).unwrap().duration_since(started);
    cancel.cancel();
    bot.await.unwrap().unwrap();
    assert!(
        to_flooder.len() <= 2,
        "флудеру ушло {} сообщений: {to_flooder:?}",
        to_flooder.len()
    );
    assert!(
        to_flooder
            .iter()
            .any(|t| t.contains("Слишком много сообщений")),
        "флудер предупреждён: {to_flooder:?}"
    );
    assert_eq!(
        count(&services, "SELECT count(*) FROM clients").await,
        2,
        "лишних клиентов нет"
    );
    assert_eq!(
        count(&services, "SELECT count(*) FROM client_identities").await,
        2
    );
    assert_eq!(
        fake.rejected.load(Ordering::SeqCst),
        0,
        "Telegram не отвечал 429"
    );
    let details: Value = {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query_scalar(
            "SELECT details FROM audit_log WHERE action = 'settings.bot_flood_blocked' ORDER BY id LIMIT 1",
        )
        .fetch_one(conn.conn())
        .await
        .unwrap()
    };
    assert_eq!(details["blocked"], 1);
    assert_eq!(details["accounts"][0]["user"], flooder);
    assert_eq!(details["accounts"][0]["name"], format!("farm{flooder}"));
    assert!(services.bot_flood().users() <= 2);
    eprintln!(
        "10 000 /start от одного аккаунта: прочитано за {:.2} с, сосед получил ответ через {:.2} с",
        elapsed.as_secs_f64(),
        neighbour_after.as_secs_f64()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn тысяча_новых_аккаунтов_упираются_в_предел_регистраций() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    for module in [ModuleId::Antibot, ModuleId::Utm] {
        services
            .modules()
            .set_enabled(services.store(), module, true, None)
            .await
            .unwrap();
    }
    flood_settings(
        &services,
        FloodSettings {
            registrations_per_minute: 100,
            ..FloodSettings::default()
        },
    )
    .await;
    let regular = 8_800_000;
    handle_update(&ctx, text_update(1, regular, "/start")).await;
    let mut updates = Vec::new();
    for round in 0..10 {
        for account in 0..1_000 {
            updates.push(start_from(0, 8_800_100 + account, Some("s_farm")));
        }
        if round == 5 {
            updates.push(json!({
                "update_id": 0,
                "callback_query": {
                    "id": "regular-press", "from": user(regular), "data": "m",
                    "message": {"message_id": 55, "chat": {"id": regular, "type": "private"}, "date": 1}
                }
            }));
        }
    }
    for (index, update) in updates.iter_mut().enumerate() {
        update["update_id"] = json!(index as i64 + 10);
    }
    let total = updates.len() as u64;
    let fake = FakeTelegram::new(updates);
    mount(&server, &fake).await;
    let started = Instant::now();
    let cancel = CancellationToken::new();
    let bot = tokio::spawn(Bot::new(services.clone(), flood_config(&server)).run(cancel.clone()));
    wait_until(
        "бот прочитал все обновления",
        Duration::from_secs(60),
        || async { fake.served.load(Ordering::SeqCst) >= total },
    )
    .await;
    wait_until(
        "обычный клиент получил ответ на кнопку",
        Duration::from_secs(20),
        || async { !fake.texts_to(regular).is_empty() },
    )
    .await;
    let regular_after = fake
        .sent
        .lock()
        .unwrap()
        .iter()
        .find(|(to, _, _)| *to == regular)
        .map(|(_, _, at)| at.duration_since(started))
        .unwrap();
    wait_until(
        "наплыв записан",
        Duration::from_secs(10),
        || async {
            count(
                &services,
                "SELECT count(*) FROM audit_log WHERE action = 'settings.bot_signups_limited'",
            )
            .await
                > 0
        },
    )
    .await;
    let elapsed = started.elapsed();
    tokio::time::sleep(Duration::from_millis(500)).await;
    cancel.cancel();
    bot.await.unwrap().unwrap();
    let clients = count(&services, "SELECT count(*) FROM clients").await;
    let allowed = 10.0 + 100.0 / 60.0 * started.elapsed().as_secs_f64() + 2.0;
    assert!(
        (10..=100).contains(&clients) && (clients as f64) <= allowed,
        "регистрации идут запасом и дальше равномерно: {clients}, можно {allowed:.0}"
    );
    let marked = count(
        &services,
        "SELECT count(*) FROM antibot_marks WHERE reasons @> '[\"surge\"]'",
    )
    .await;
    assert!(
        marked < clients,
        "отмечены только принятые во время наплыва: {marked} из {clients}"
    );
    assert_eq!(
        fake.rejected.load(Ordering::SeqCst),
        0,
        "Telegram не отвечал 429"
    );
    let busy = fake
        .sent
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, text, _)| text.contains("слишком много новых пользователей"))
        .count();
    let farm_replies = fake
        .sent
        .lock()
        .unwrap()
        .iter()
        .filter(|(to, _, _)| *to >= 8_800_100)
        .count();
    assert!(
        farm_replies <= 1_000,
        "каждому не больше одного ответа: {farm_replies}"
    );
    let clicks = count(
        &services,
        "SELECT coalesce(sum(clicks), 0)::bigint FROM utm_clicks",
    )
    .await;
    assert!(
        (1..=1_000).contains(&clicks),
        "переход по метке считается один раз на аккаунт: {clicks}"
    );
    assert!(services.bot_flood().users() <= 1_001);
    eprintln!(
        "10 000 /start от 1000 аккаунтов: прочитано за {:.2} с, ответ на кнопку через {:.2} с, клиентов {clients}, «попробуйте позже» {busy}, всего ответов ферме {farm_replies}",
        elapsed.as_secs_f64(),
        regular_after.as_secs_f64()
    );
    db.cleanup().await;
}
