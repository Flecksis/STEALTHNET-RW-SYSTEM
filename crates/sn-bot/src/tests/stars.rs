use super::*;
use crate::stars as ledger;
use sn_domain::ids::PaymentId;
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::order::{NewOrder, OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::payment::{NewPayment, Payment, PaymentStatus, ProviderId, providers};
use sn_domain::pricing::{QuoteRequest, quote};
use sn_services::{Clock, FixedClock};
use std::sync::Mutex;
use wiremock::{Request, Respond};

struct Updates {
    updates: Vec<serde_json::Value>,
}

impl Respond for Updates {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let offset = body["offset"].as_i64().unwrap_or(0);
        let fresh: Vec<serde_json::Value> = self
            .updates
            .iter()
            .filter(|u| u["update_id"].as_i64().unwrap() >= offset)
            .cloned()
            .collect();
        let delay = if fresh.is_empty() { 100 } else { 0 };
        ok(serde_json::json!(fresh)).set_delay(Duration::from_millis(delay))
    }
}

#[derive(Clone, Default)]
pub(super) struct Ledger {
    transactions: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Ledger {
    pub(super) fn push(&self, transaction: serde_json::Value) {
        self.transactions.lock().unwrap().push(transaction);
    }
}

impl Respond for Ledger {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let offset = body["offset"].as_u64().unwrap_or(0) as usize;
        let limit = body["limit"].as_u64().unwrap_or(100) as usize;
        let page: Vec<serde_json::Value> = self
            .transactions
            .lock()
            .unwrap()
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();
        ok(serde_json::json!({ "transactions": page }))
    }
}

pub(super) fn incoming(charge: &str, from: i64, payload: &str, stars: i64) -> serde_json::Value {
    serde_json::json!({
        "id": charge, "amount": stars, "date": 1_763_300_000,
        "source": {
            "type": "user", "transaction_type": "invoice_payment",
            "user": {"id": from, "is_bot": false, "first_name": "Алексей"},
            "invoice_payload": payload
        }
    })
}

pub(super) fn refund(charge: &str, to: i64, stars: i64) -> serde_json::Value {
    serde_json::json!({
        "id": charge, "amount": stars, "date": 1_763_400_000,
        "receiver": {
            "type": "user", "transaction_type": "invoice_payment",
            "user": {"id": to, "is_bot": false, "first_name": "Алексей"}
        }
    })
}

pub(super) fn paid_update(
    update_id: i64,
    from: i64,
    payment: &Payment,
    charge: &str,
) -> serde_json::Value {
    serde_json::json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id, "from": user(from),
            "chat": {"id": from, "type": "private"}, "date": 1,
            "successful_payment": {
                "currency": "XTR",
                "total_amount": payment.amount.minor,
                "invoice_payload": payment.public_id.to_string(),
                "telegram_payment_charge_id": charge,
                "provider_payment_charge_id": ""
            }
        }
    })
}

pub(super) async fn serve_updates(server: &MockServer, updates: Vec<serde_json::Value>) {
    Mock::given(method("POST"))
        .and(path_regex(r"/getUpdates$"))
        .respond_with(Updates { updates })
        .mount(server)
        .await;
}

pub(super) async fn serve_ledger(server: &MockServer, ledger: &Ledger) {
    Mock::given(method("POST"))
        .and(path_regex(r"/getStarTransactions$"))
        .respond_with(ledger.clone())
        .mount(server)
        .await;
}

pub(super) fn quick(server: &MockServer) -> BotConfig {
    BotConfig {
        api_base: server.uri(),
        poll_timeout: Duration::from_secs(1),
        workers: 2,
        stars_retry_every: Duration::from_millis(100),
        stars_reconcile_every: Duration::from_millis(200),
    }
}

pub(super) async fn stars_invoice(db: &TestDb, services: &Services, telegram_id: i64) -> Payment {
    let (client, _) = services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: telegram_id,
                username: None,
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
    let mut tx = db.store.begin().await.unwrap();
    let category = tx
        .tariffs()
        .create_category(&NewCategory {
            name: format!("Основные {telegram_id}"),
            sort: 0,
            visible: true,
        })
        .await
        .unwrap();
    let tariff = tx
        .tariffs()
        .create(&NewTariff {
            category_id: category.id,
            name: format!("Стандарт {telegram_id}"),
            description: None,
            on_sale: true,
            sort: 0,
            devices: DevicePolicy {
                included: 3,
                extra: None,
            },
            traffic: TrafficPolicy::UNLIMITED,
            squads: vec![sn_domain::ids::SquadId(uuid::Uuid::from_u128(1))],
            external_squad: None,
            periods: vec![NewPeriod {
                days: 90,
                prices: CurrencyPrices {
                    usd: Some(699),
                    rub: Some(49_900),
                },
                stars: Some(Stars(350)),
                sort: 0,
                active: true,
            }],
        })
        .await
        .unwrap();
    let period = tariff.periods[0].clone();
    let priced = quote(&QuoteRequest {
        currency: Currency::Usd,
        tariff: &tariff,
        period_id: period.id,
        extra_devices: 0,
        sell_extra_devices: false,
        adjustments: &[],
    })
    .unwrap();
    let order = tx
        .orders()
        .create(&NewOrder {
            client_id: client.id,
            subscription_id: None,
            kind: OrderKind::New,
            intent: PurchaseIntent::Subscription {
                tariff_id: tariff.id,
                period_id: period.id,
                extra_devices: 0,
                target: None,
            },
            quote: priced,
            snapshot: Some(tariff.snapshot(&period)),
        })
        .await
        .unwrap();
    let payment = tx
        .payments()
        .create(&NewPayment {
            order_id: order.id,
            client_id: client.id,
            provider: ProviderId::from(providers::STARS),
            method: None,
            amount: Amount::new(350, PayCurrency::Xtr),
            expires_at: None,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    payment
}

pub(super) async fn payment_of(db: &TestDb, id: PaymentId) -> Payment {
    let mut conn = db.store.conn().await.unwrap();
    conn.payments().get(id).await.unwrap()
}

pub(super) async fn order_status(db: &TestDb, payment: &Payment) -> OrderStatus {
    let mut conn = db.store.conn().await.unwrap();
    conn.orders().get(payment.order_id).await.unwrap().status
}

pub(super) async fn wait_status(db: &TestDb, id: PaymentId, status: PaymentStatus) -> bool {
    for _ in 0..100 {
        if payment_of(db, id).await.status == status {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn arm_faults(db: &TestDb, tables: &[&str]) {
    let mut conn = db.store.conn().await.unwrap();
    for sql in [
        "CREATE TABLE test_faults (target text PRIMARY KEY)",
        "CREATE FUNCTION test_fault() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN \
           IF EXISTS (SELECT 1 FROM test_faults WHERE target = TG_TABLE_NAME) THEN \
             RAISE EXCEPTION 'сбой базы' USING ERRCODE = '40001'; \
           END IF; \
           RETURN NEW; \
         END $$",
    ] {
        sqlx::query(sql).execute(conn.conn()).await.unwrap();
    }
    for table in tables {
        sqlx::query(&format!(
            "CREATE TRIGGER {table}_fault BEFORE INSERT OR UPDATE ON {table} \
             FOR EACH ROW EXECUTE FUNCTION test_fault()"
        ))
        .execute(conn.conn())
        .await
        .unwrap();
    }
}

async fn fault(db: &TestDb, target: &str, on: bool) {
    let mut conn = db.store.conn().await.unwrap();
    let sql = if on {
        "INSERT INTO test_faults (target) VALUES ($1) ON CONFLICT DO NOTHING"
    } else {
        "DELETE FROM test_faults WHERE target = $1"
    };
    sqlx::query(sql)
        .bind(target)
        .execute(conn.conn())
        .await
        .unwrap();
}

async fn told(server: &MockServer, needle: &str) -> bool {
    for _ in 0..100 {
        if sent(server)
            .await
            .iter()
            .any(|b| b["text"].as_str().is_some_and(|t| t.contains(needle)))
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn сбой_базы_при_проведении_оплаты_звёздами_не_теряет_её() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    let payment = stars_invoice(&db, &services, 730).await;
    arm_faults(&db, &["payments"]).await;
    fault(&db, "payments", true).await;
    serve_updates(&server, vec![paid_update(61, 730, &payment, "stx-730")]).await;
    let cancel = CancellationToken::new();
    let running = tokio::spawn(Bot::new(services.clone(), quick(&server)).run(cancel.clone()));
    let warned = told(&server, "не смогли сразу").await;
    let still_pending = payment_of(&db, payment.id).await.status == PaymentStatus::Pending;
    fault(&db, "payments", false).await;
    let recovered = wait_status(&db, payment.id, PaymentStatus::Paid).await;
    cancel.cancel();
    running.await.unwrap().unwrap();
    assert!(
        warned,
        "клиенту не сказали, что оплата будет проведена позже"
    );
    assert!(still_pending);
    assert!(recovered, "оплата звёздами потерялась после сбоя базы");
    let paid = payment_of(&db, payment.id).await;
    assert_eq!(paid.provider_txid.as_deref(), Some("stx-730"));
    assert_eq!(order_status(&db, &paid).await, OrderStatus::Paid);
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(
        conn.bot_state().get(OFFSET_KEY).await.unwrap(),
        Some(serde_json::json!(62))
    );
    drop(conn);
    let event = stars_event(paid_update(61, 730, &payment, "stx-730"));
    assert_eq!(
        ledger::status(&services, &event).await.unwrap(),
        Some(ledger::Status::Done)
    );
    db.cleanup().await;
}

pub(super) fn stars_event(update: serde_json::Value) -> sn_payments::providers::StarsPayment {
    let update: Update = serde_json::from_value(update).unwrap();
    ledger::from_updates(std::slice::from_ref(&update))
        .pop()
        .unwrap()
}

pub(super) fn refund_update(
    update_id: i64,
    from: i64,
    payment: &Payment,
    charge: &str,
) -> serde_json::Value {
    serde_json::json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id, "from": user(from),
            "chat": {"id": from, "type": "private"}, "date": 1,
            "refunded_payment": {
                "currency": "XTR",
                "total_amount": payment.amount.minor,
                "invoice_payload": payment.public_id.to_string(),
                "telegram_payment_charge_id": charge
            }
        }
    })
}

async fn get_updates_offsets(server: &MockServer) -> Vec<serde_json::Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().ends_with("/getUpdates"))
        .map(|r| serde_json::from_slice::<serde_json::Value>(&r.body).unwrap()["offset"].clone())
        .collect()
}

pub(super) async fn reviews(db: &TestDb, payment: PaymentId) -> i64 {
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE entity_kind = 'payment' \
         AND (entity_id = $1 OR entity_id IN \
             (SELECT payment_id FROM payment_repeats WHERE original_payment_id = $1)) \
         AND action IN ('payment.stars_review', 'payment.repeat_review')",
    )
    .bind(payment.0)
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

#[tokio::test]
async fn оплата_звёздами_сохраняется_до_сдвига_смещения() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    let payment = stars_invoice(&db, &services, 734).await;
    arm_faults(&db, &["bot_star_events"]).await;
    fault(&db, "bot_star_events", true).await;
    serve_updates(
        &server,
        vec![
            serde_json::to_value(text_update(70, 735, "/start")).unwrap(),
            paid_update(71, 734, &payment, "stx-734"),
        ],
    )
    .await;
    let cancel = CancellationToken::new();
    let running = tokio::spawn(Bot::new(services.clone(), quick(&server)).run(cancel.clone()));
    let mut polls = Vec::new();
    for _ in 0..100 {
        polls = get_updates_offsets(&server).await;
        if polls.len() >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let untouched = client_of(&services, 735).await.is_none();
    let pending = payment_of(&db, payment.id).await.status == PaymentStatus::Pending;
    let unsaved = {
        let mut conn = db.store.conn().await.unwrap();
        conn.bot_state().get(OFFSET_KEY).await.unwrap().is_none()
    };
    fault(&db, "bot_star_events", false).await;
    let recovered = wait_status(&db, payment.id, PaymentStatus::Paid).await;
    let mut registered = false;
    for _ in 0..50 {
        if client_of(&services, 735).await.is_some() {
            registered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    cancel.cancel();
    running.await.unwrap().unwrap();
    assert!(polls.len() >= 3, "{polls:?}");
    assert!(
        polls.iter().all(serde_json::Value::is_null),
        "смещение сдвинулось, хотя оплата не сохранена: {polls:?}"
    );
    assert!(untouched, "апдейты пачки обработаны до сохранения оплаты");
    assert!(pending);
    assert!(unsaved);
    assert!(recovered, "оплата звёздами потерялась");
    assert!(registered);
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(
        conn.bot_state().get(OFFSET_KEY).await.unwrap(),
        Some(serde_json::json!(72))
    );
    drop(conn);
    assert_eq!(
        ledger::status(
            &services,
            &stars_event(paid_update(71, 734, &payment, "stx-734"))
        )
        .await
        .unwrap(),
        Some(ledger::Status::Done)
    );
    db.cleanup().await;
}

#[tokio::test]
async fn сохранённая_оплата_проводится_даже_если_обработчик_не_успел() {
    let Some(db) = test_store().await else {
        return;
    };
    let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::new(catalog::all()).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks {
            bot_token: Some(TOKEN.into()),
            ..EnvFallbacks::default()
        },
    });
    services.reload().await.unwrap();
    let payment = stars_invoice(&db, &services, 740).await;
    let updates: Vec<Update> = [
        paid_update(90, 740, &payment, "stx-740"),
        refund_update(91, 740, &payment, "stx-740"),
        serde_json::to_value(text_update(92, 740, "/start")).unwrap(),
    ]
    .into_iter()
    .map(|u| serde_json::from_value(u).unwrap())
    .collect();
    let events = ledger::from_updates(&updates);
    assert_eq!(events.len(), 2);
    assert_eq!(ledger::record(&services, &events).await.unwrap(), 2);
    assert_eq!(ledger::record(&services, &events).await.unwrap(), 0);
    let base = Duration::from_secs(30);
    assert_eq!(
        ledger::retry_due(&services, base).await.unwrap(),
        ledger::Retried::default(),
        "обработчику дали время провести оплату самому"
    );
    clock.set(clock.now() + chrono::Duration::seconds(61));
    assert_eq!(
        ledger::retry_due(&services, base).await.unwrap(),
        ledger::Retried {
            taken: 1,
            applied: 1
        },
        "возврат ждёт, пока проведётся оплата"
    );
    assert_eq!(
        payment_of(&db, payment.id).await.status,
        PaymentStatus::Paid
    );
    assert_eq!(
        ledger::retry_due(&services, base).await.unwrap(),
        ledger::Retried {
            taken: 1,
            applied: 1
        }
    );
    let refunded = payment_of(&db, payment.id).await;
    assert_eq!(refunded.status, PaymentStatus::Refunded);
    assert_eq!(order_status(&db, &refunded).await, OrderStatus::Refunded);
    for event in &events {
        assert_eq!(
            ledger::status(&services, event).await.unwrap(),
            Some(ledger::Status::Done)
        );
    }
    assert_eq!(
        ledger::retry_due(&services, base).await.unwrap(),
        ledger::Retried::default()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn расхождение_по_оплате_звёздами_уходит_на_ручную_проверку() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let payment = stars_invoice(&db, &services, 741).await;
    let first = paid_update(93, 741, &payment, "stx-741-a");
    handle_update(&ctx, serde_json::from_value(first.clone()).unwrap()).await;
    assert_eq!(
        payment_of(&db, payment.id).await.status,
        PaymentStatus::Paid
    );
    assert!(
        last_text(&server)
            .await
            .contains("Оплата звёздами получена")
    );
    assert_eq!(
        ledger::status(&services, &stars_event(first))
            .await
            .unwrap(),
        Some(ledger::Status::Done)
    );
    handle_update(
        &ctx,
        serde_json::from_value(paid_update(94, 741, &payment, "stx-741-b")).unwrap(),
    )
    .await;
    assert!(
        reviews(&db, payment.id).await > 0,
        "вторая оплата того же счёта прошла молча"
    );
    assert!(
        last_text(&server).await.contains("уже был оплачен"),
        "бот пообещал обновить подписку за повторную оплату"
    );
    let short = stars_invoice(&db, &services, 742).await;
    let mut underpaid = paid_update(95, 742, &short, "stx-742");
    underpaid["message"]["successful_payment"]["total_amount"] = serde_json::json!(349);
    handle_update(&ctx, serde_json::from_value(underpaid.clone()).unwrap()).await;
    assert_eq!(
        payment_of(&db, short.id).await.status,
        PaymentStatus::Pending
    );
    assert!(last_text(&server).await.contains("не смогли сразу"));
    assert_eq!(
        ledger::status(&services, &stars_event(underpaid))
            .await
            .unwrap(),
        Some(ledger::Status::Review)
    );
    assert_eq!(reviews(&db, short.id).await, 1);
    assert_eq!(
        ledger::retry_due(&services, Duration::from_millis(1))
            .await
            .unwrap(),
        ledger::Retried::default()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn сверка_транзакций_звёзд_находит_пропущенные_оплату_и_возврат() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    let lost = stars_invoice(&db, &services, 731).await;
    let returned = stars_invoice(&db, &services, 732).await;
    let later = stars_invoice(&db, &services, 733).await;
    serve_updates(&server, Vec::new()).await;
    let ledger = Ledger::default();
    ledger.push(incoming("legacy-1", 900, "snstars:17", 120));
    ledger.push(incoming(
        "charge-731",
        731,
        &lost.public_id.to_string(),
        350,
    ));
    ledger.push(incoming(
        "charge-732",
        732,
        &returned.public_id.to_string(),
        350,
    ));
    ledger.push(refund("charge-732", 732, 350));
    ledger.push(serde_json::json!({
        "id": "withdrawal-1", "amount": 1000, "date": 1_763_500_000,
        "receiver": {"type": "fragment"}
    }));
    serve_ledger(&server, &ledger).await;
    let cancel = CancellationToken::new();
    let running = tokio::spawn(Bot::new(services.clone(), quick(&server)).run(cancel.clone()));
    let found = wait_status(&db, lost.id, PaymentStatus::Paid).await;
    let reverted = wait_status(&db, returned.id, PaymentStatus::Refunded).await;
    cancel.cancel();
    running.await.unwrap().unwrap();
    assert!(found, "сверка не нашла оплату, которую бот пропустил");
    assert!(reverted, "сверка не провела возврат звёзд");
    let paid = payment_of(&db, lost.id).await;
    assert_eq!(paid.provider_txid.as_deref(), Some("charge-731"));
    assert_eq!(order_status(&db, &paid).await, OrderStatus::Paid);
    assert_eq!(
        order_status(&db, &payment_of(&db, returned.id).await).await,
        OrderStatus::Refunded
    );
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(
        conn.bot_state()
            .get("telegram.stars_offset.1")
            .await
            .unwrap(),
        Some(serde_json::json!(5))
    );
    assert_eq!(
        conn.refunds().for_payment(returned.id).await.unwrap().len(),
        1
    );
    drop(conn);
    ledger.push(incoming(
        "charge-733",
        733,
        &later.public_id.to_string(),
        350,
    ));
    let before = server.received_requests().await.unwrap().len();
    let cancel = CancellationToken::new();
    let running = tokio::spawn(Bot::new(services.clone(), quick(&server)).run(cancel.clone()));
    let caught_up = wait_status(&db, later.id, PaymentStatus::Paid).await;
    cancel.cancel();
    running.await.unwrap().unwrap();
    assert!(
        caught_up,
        "после перезапуска сверка не продолжила с курсора"
    );
    let offsets: Vec<u64> = server.received_requests().await.unwrap()[before..]
        .iter()
        .filter(|r| r.url.path().ends_with("/getStarTransactions"))
        .map(|r| {
            serde_json::from_slice::<serde_json::Value>(&r.body).unwrap()["offset"]
                .as_u64()
                .unwrap()
        })
        .collect();
    assert_eq!(offsets.first(), Some(&5), "{offsets:?}");
    assert!(offsets.iter().all(|o| *o >= 5), "{offsets:?}");
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(
        conn.bot_state()
            .get("telegram.stars_offset.1")
            .await
            .unwrap(),
        Some(serde_json::json!(6))
    );
    drop(conn);
    for payment in [&lost, &returned, &later] {
        assert_ne!(
            payment_of(&db, payment.id).await.status,
            PaymentStatus::Pending
        );
    }
    db.cleanup().await;
}
