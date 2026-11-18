use super::stars::{
    Ledger, incoming, order_status, paid_update, payment_of, refund, refund_update, reviews,
    serve_ledger, stars_event, stars_invoice,
};
use super::*;
use crate::stars as ledger;
use sn_domain::ids::AdminId;
use sn_domain::money::{Amount, PayCurrency};
use sn_domain::order::OrderStatus;
use sn_domain::paging::PageRequest;
use sn_domain::payment::{Payment, PaymentStatus};
use sn_payments::providers::{STARS_TOKEN_FIELD, StarsPaymentEvent, TelegramStars};
use sn_payments::{PaymentError, ProviderConfig};
use sn_services::ServiceError;
use sn_services::admin::MarkPaidRequest;
use sn_services::stars_ledger::REFUNDED_TEMPLATE;
use sn_services::stars_review::{
    REFUND_FAILED_ACTION, REFUNDED_ACTION, RETRIED_ACTION, StarsResolutionKind, StarsRetryOutcome,
    StarsReview,
};
use wiremock::matchers::{body_json, path};

fn bot_config() -> ProviderConfig {
    let mut config = ProviderConfig::default();
    config.set(STARS_TOKEN_FIELD, serde_json::json!(TOKEN));
    config
}

fn underpaid_update(
    update_id: i64,
    from: i64,
    payment: &Payment,
    charge: &str,
    stars: i64,
) -> serde_json::Value {
    let mut update = paid_update(update_id, from, payment, charge);
    update["message"]["successful_payment"]["total_amount"] = serde_json::json!(stars);
    update
}

fn refund_echo(
    update_id: i64,
    from: i64,
    payment: &Payment,
    charge: &str,
    stars: i64,
) -> serde_json::Value {
    let mut update = refund_update(update_id, from, payment, charge);
    update["message"]["refunded_payment"]["total_amount"] = serde_json::json!(stars);
    update
}

async fn owner(services: &Services) -> AdminId {
    services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap()
        .0
}

fn refund_path() -> String {
    format!("/bot{TOKEN}/refundStarPayment")
}

async fn refund_calls(server: &MockServer) -> Vec<serde_json::Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().ends_with("/refundStarPayment"))
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

async fn audit_of(db: &TestDb, action: &str) -> Vec<sn_store::AuditRecord> {
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query_as(
        "SELECT id, actor_kind, actor_id, action, entity_kind, entity_id, details, created_at \
         FROM audit_log WHERE action = $1 ORDER BY id",
    )
    .bind(action)
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

async fn stars_notices(db: &TestDb) -> Vec<(ClientId, serde_json::Value)> {
    let mut conn = db.store.conn().await.unwrap();
    let rows: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT payload FROM jobs WHERE kind = 'notify' ORDER BY id")
            .fetch_all(conn.conn())
            .await
            .unwrap();
    rows.into_iter()
        .filter_map(|payload| serde_json::from_value::<JobPayload>(payload).ok())
        .filter_map(|job| match job {
            JobPayload::Notify {
                client_id,
                template,
                params,
            } if template == REFUNDED_TEMPLATE => Some((client_id, params)),
            _ => None,
        })
        .collect()
}

async fn fulfil_jobs(db: &TestDb) -> i64 {
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'fulfill_order'")
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

async fn seed_failed(db: &TestDb, payment: &Payment, update: &serde_json::Value) -> i64 {
    let event = stars_event(update.clone());
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query_scalar(
        "INSERT INTO bot_star_events \
         (charge_id, event, payload, source, status, attempts, next_attempt_at, last_error, payment_id) \
         VALUES ($1, 'successful_payment', $2, 'update', 'review', 1, now(), $3, $4) RETURNING id",
    )
    .bind(&event.telegram_payment_charge_id)
    .bind(serde_json::to_value(&event).unwrap())
    .bind("модуль не ответил")
    .bind(payment.id.0)
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

async fn under_review(
    db: &TestDb,
    services: &Services,
    ctx: &BotCtx,
    telegram_id: i64,
    charge: &str,
) -> (Payment, serde_json::Value, i64) {
    let payment = stars_invoice(db, services, telegram_id).await;
    let update = underpaid_update(telegram_id, telegram_id, &payment, charge, 349);
    handle_update(ctx, serde_json::from_value(update.clone()).unwrap()).await;
    assert_eq!(
        ledger::status(services, &stars_event(update.clone()))
            .await
            .unwrap(),
        Some(ledger::Status::Review)
    );
    let page = StarsReview::new(services)
        .list(PageRequest::default())
        .await
        .unwrap();
    let id = page
        .items
        .iter()
        .find(|item| item.charge_id == charge)
        .map(|item| item.id)
        .unwrap();
    (payment, update, id)
}

#[tokio::test]
async fn возврат_непроведённого_списания_не_трогает_платёж_оплаченный_другим_списанием() {
    let Some((db, services, _server, ctx)) = setup().await else {
        return;
    };
    let payment = stars_invoice(&db, &services, 750).await;
    let underpaid = underpaid_update(110, 750, &payment, "stx-750-a", 349);
    handle_update(&ctx, serde_json::from_value(underpaid.clone()).unwrap()).await;
    assert_eq!(
        ledger::status(&services, &stars_event(underpaid))
            .await
            .unwrap(),
        Some(ledger::Status::Review)
    );
    handle_update(
        &ctx,
        serde_json::from_value(paid_update(111, 750, &payment, "stx-750-b")).unwrap(),
    )
    .await;
    let paid = payment_of(&db, payment.id).await;
    assert_eq!(paid.status, PaymentStatus::Paid);
    assert_eq!(paid.provider_txid.as_deref(), Some("stx-750-b"));
    let echo = refund_echo(112, 750, &payment, "stx-750-a", 349);
    handle_update(&ctx, serde_json::from_value(echo.clone()).unwrap()).await;
    let after = payment_of(&db, payment.id).await;
    assert_eq!(
        after.status,
        PaymentStatus::Paid,
        "возврат списания, которое не проводилось, записан на платёж с другим списанием"
    );
    assert_eq!(order_status(&db, &after).await, OrderStatus::Paid);
    let mut conn = db.store.conn().await.unwrap();
    assert!(
        conn.refunds()
            .for_payment(payment.id)
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    assert_eq!(
        ledger::status(&services, &stars_event(echo)).await.unwrap(),
        Some(ledger::Status::Done)
    );
    db.cleanup().await;
}

#[tokio::test]
async fn сверка_не_записывает_возврат_непроведённого_списания_на_оплаченный_платёж() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let payment = stars_invoice(&db, &services, 752).await;
    let underpaid = underpaid_update(120, 752, &payment, "stx-752-a", 349);
    handle_update(&ctx, serde_json::from_value(underpaid.clone()).unwrap()).await;
    handle_update(
        &ctx,
        serde_json::from_value(paid_update(121, 752, &payment, "stx-752-b")).unwrap(),
    )
    .await;
    assert_eq!(
        payment_of(&db, payment.id).await.status,
        PaymentStatus::Paid
    );
    let transactions = Ledger::default();
    let payload = payment.public_id.to_string();
    transactions.push(incoming("stx-752-a", 752, &payload, 349));
    transactions.push(incoming("stx-752-b", 752, &payload, 350));
    transactions.push(refund("stx-752-a", 752, 349));
    serve_ledger(&server, &transactions).await;
    let stars = TelegramStars::with_api_base(server.uri());
    let report = ledger::reconcile(&services, &stars, &bot_config(), 1)
        .await
        .unwrap();
    assert_eq!(report.read, 3);
    assert_eq!(
        report.recorded, 1,
        "возврат из сверки должен попасть в журнал"
    );
    let retried = ledger::retry_due(&services, Duration::from_millis(1))
        .await
        .unwrap();
    assert_eq!(retried.taken, 1);
    let after = payment_of(&db, payment.id).await;
    assert_eq!(
        after.status,
        PaymentStatus::Paid,
        "сверка записала возврат чужого списания на оплаченный платёж"
    );
    assert_eq!(order_status(&db, &after).await, OrderStatus::Paid);
    assert_eq!(
        ledger::status(&services, &stars_event(underpaid))
            .await
            .unwrap(),
        Some(ledger::Status::Review),
        "событие на ручной проверке осталось ждать решения"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn возврат_звёзд_по_событию_на_проверке_идёт_в_telegram_один_раз_и_закрывает_событие() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let admin = owner(&services).await;
    let (payment, update, id) = under_review(&db, &services, &ctx, 760, "stx-760").await;
    let telegram = TelegramStars::with_api_base(server.uri());
    let review = StarsReview::with_telegram(&services, &telegram);
    let page = review.list(PageRequest::default()).await.unwrap();
    assert_eq!(page.items.len(), 1);
    let item = &page.items[0];
    assert_eq!(item.id, id);
    assert_eq!(item.charge_id, "stx-760");
    assert_eq!(item.event, StarsPaymentEvent::SuccessfulPayment);
    assert_eq!(item.stars, Some(Stars(349)));
    assert_eq!(item.telegram_user_id, Some(760));
    assert_eq!(item.payer_client_id, Some(payment.client_id));
    let linked = item.payment.as_ref().unwrap();
    assert_eq!(linked.id, payment.id);
    assert_eq!(linked.status, PaymentStatus::Pending);
    assert_eq!(linked.amount, Amount::new(350, PayCurrency::Xtr));
    assert_eq!(item.order.unwrap().id, payment.order_id);
    assert_eq!(item.order.unwrap().status, OrderStatus::Pending);
    assert!(
        item.reason
            .as_deref()
            .is_some_and(|reason| reason.contains("сумма не сошлась")),
        "{:?}",
        item.reason
    );
    assert!(item.refundable);
    assert!(!item.telegram_refunded);
    Mock::given(method("POST"))
        .and(path(refund_path()))
        .and(body_json(serde_json::json!({
            "user_id": 760, "telegram_payment_charge_id": "stx-760"
        })))
        .respond_with(ok(serde_json::json!(true)))
        .expect(1)
        .mount(&server)
        .await;
    let done = review.refund(id, Some(" недоплата "), admin).await.unwrap();
    assert_eq!(done.status, StarsResolutionKind::Refunded);
    assert!(!done.repeated);
    assert!(!done.already_refunded);
    assert_eq!(done.charge_id, "stx-760");
    assert_eq!(done.stars, Some(Stars(349)));
    assert_eq!(done.telegram_user_id, Some(760));
    assert_eq!(done.payment_id, Some(payment.id));
    assert_eq!(done.resolved_by, Some(admin));
    assert_eq!(done.comment.as_deref(), Some("недоплата"));
    assert_eq!(
        ledger::status(&services, &stars_event(update.clone()))
            .await
            .unwrap(),
        Some(ledger::Status::Refunded)
    );
    assert!(
        review
            .list(PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let audit = audit_of(&db, REFUNDED_ACTION).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].actor_kind, "admin");
    assert_eq!(audit[0].actor_id, Some(admin.0));
    assert_eq!(audit[0].entity_kind.as_deref(), Some("payment"));
    assert_eq!(audit[0].entity_id, Some(payment.id.0));
    assert_eq!(audit[0].details["event_id"], id);
    assert_eq!(audit[0].details["charge_id"], "stx-760");
    assert_eq!(audit[0].details["stars"], 349);
    assert_eq!(audit[0].details["telegram_user_id"], 760);
    assert_eq!(audit[0].details["comment"], "недоплата");
    assert_eq!(audit[0].details["already_refunded"], false);
    let notices = stars_notices(&db).await;
    assert_eq!(notices.len(), 1, "клиенту не написали о возврате звёзд");
    assert_eq!(notices[0].0, payment.client_id);
    assert_eq!(notices[0].1["stars"], 349);
    assert_eq!(notices[0].1["charge_id"], "stx-760");
    assert_eq!(notices[0].1["label"], "Стандарт 760 · 90\u{a0}дней");
    assert_eq!(notices[0].1["label_en"], "Стандарт 760 · 90\u{a0}days");
    let again = review.refund(id, Some("ещё раз"), admin).await.unwrap();
    assert!(again.repeated);
    assert_eq!(again.status, StarsResolutionKind::Refunded);
    assert_eq!(again.comment.as_deref(), Some("недоплата"));
    assert_eq!(again.resolved_at, done.resolved_at);
    assert_eq!(
        refund_calls(&server).await.len(),
        1,
        "повторный возврат снова обратился к Telegram"
    );
    assert_eq!(audit_of(&db, REFUNDED_ACTION).await.len(), 1);
    assert_eq!(
        review
            .close(id, "уже возвращено", admin)
            .await
            .unwrap_err()
            .code(),
        "stars.review_refunded"
    );
    let echo = refund_echo(761, 760, &payment, "stx-760", 349);
    handle_update(&ctx, serde_json::from_value(echo.clone()).unwrap()).await;
    assert_eq!(
        ledger::status(&services, &stars_event(echo)).await.unwrap(),
        Some(ledger::Status::Done)
    );
    assert_eq!(
        stars_notices(&db).await.len(),
        1,
        "сообщение Telegram о том же возврате не должно дублировать письмо клиенту"
    );
    handle_update(&ctx, serde_json::from_value(update.clone()).unwrap()).await;
    assert_eq!(
        ledger::status(&services, &stars_event(update))
            .await
            .unwrap(),
        Some(ledger::Status::Refunded),
        "повтор апдейта снова провёл возвращённую оплату"
    );
    assert_eq!(reviews(&db, payment.id).await, 1);
    let untouched = payment_of(&db, payment.id).await;
    assert_eq!(untouched.status, PaymentStatus::Pending);
    assert_eq!(untouched.provider_txid, None);
    assert_eq!(order_status(&db, &untouched).await, OrderStatus::Pending);
    let mut conn = db.store.conn().await.unwrap();
    assert!(
        conn.refunds()
            .for_payment(payment.id)
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    assert_eq!(
        services
            .admin()
            .mark_paid(payment.id, &MarkPaidRequest::default(), admin)
            .await
            .unwrap_err()
            .code(),
        "payment.manual_stars"
    );
    assert_eq!(
        ledger::retry_due(&services, Duration::from_millis(1))
            .await
            .unwrap(),
        ledger::Retried::default()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn если_telegram_уже_вернул_звёзды_событие_закрывается_без_ошибки() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let admin = owner(&services).await;
    let (payment, update, id) = under_review(&db, &services, &ctx, 762, "stx-762").await;
    Mock::given(method("POST"))
        .and(path(refund_path()))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "ok": false, "error_code": 400, "description": "Bad Request: CHARGE_ALREADY_REFUNDED"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let telegram = TelegramStars::with_api_base(server.uri());
    let review = StarsReview::with_telegram(&services, &telegram);
    let done = review.refund(id, None, admin).await.unwrap();
    assert_eq!(done.status, StarsResolutionKind::Refunded);
    assert!(done.already_refunded);
    assert!(!done.repeated);
    assert_eq!(done.comment, None);
    assert_eq!(
        ledger::status(&services, &stars_event(update))
            .await
            .unwrap(),
        Some(ledger::Status::Refunded)
    );
    let audit = audit_of(&db, REFUNDED_ACTION).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].details["already_refunded"], true);
    assert_eq!(audit[0].entity_id, Some(payment.id.0));
    assert!(audit_of(&db, REFUND_FAILED_ACTION).await.is_empty());
    assert!(review.refund(id, None, admin).await.unwrap().repeated);
    assert_eq!(refund_calls(&server).await.len(), 1);
    assert_eq!(
        stars_notices(&db).await.len(),
        1,
        "уведомления Telegram о возврате ещё не было: сообщение ставит админка"
    );
    handle_update(
        &ctx,
        serde_json::from_value(refund_echo(763, 762, &payment, "stx-762", 349)).unwrap(),
    )
    .await;
    assert_eq!(stars_notices(&db).await.len(), 1);
    db.cleanup().await;
}

#[tokio::test]
async fn после_уведомления_telegram_о_возврате_админка_не_пишет_второй_раз() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let admin = owner(&services).await;
    let (payment, _update, id) = under_review(&db, &services, &ctx, 764, "stx-764").await;
    handle_update(
        &ctx,
        serde_json::from_value(refund_echo(765, 764, &payment, "stx-764", 349)).unwrap(),
    )
    .await;
    assert_eq!(stars_notices(&db).await.len(), 1);
    Mock::given(method("POST"))
        .and(path(refund_path()))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "ok": false, "error_code": 400, "description": "Bad Request: CHARGE_ALREADY_REFUNDED"
        })))
        .mount(&server)
        .await;
    let telegram = TelegramStars::with_api_base(server.uri());
    let review = StarsReview::with_telegram(&services, &telegram);
    let done = review.refund(id, None, admin).await.unwrap();
    assert!(done.already_refunded);
    assert_eq!(stars_notices(&db).await.len(), 1);
    db.cleanup().await;
}

#[tokio::test]
async fn отказ_telegram_оставляет_событие_на_проверке_а_повтор_возвращает_звёзды() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let admin = owner(&services).await;
    let (payment, update, id) = under_review(&db, &services, &ctx, 763, "stx-763").await;
    Mock::given(method("POST"))
        .and(path(refund_path()))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(refund_path()))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "ok": false, "error_code": 400, "description": "Bad Request: USER_ID_INVALID"
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(refund_path()))
        .respond_with(ok(serde_json::json!(true)))
        .mount(&server)
        .await;
    let telegram = TelegramStars::with_api_base(server.uri());
    let review = StarsReview::with_telegram(&services, &telegram);
    let unavailable = review.refund(id, Some("первая попытка"), admin).await;
    assert!(
        matches!(
            unavailable,
            Err(ServiceError::Payment(PaymentError::Http { status: 500 }))
        ),
        "{unavailable:?}"
    );
    let refused = review.refund(id, Some("вторая попытка"), admin).await;
    match refused {
        Err(ServiceError::Payment(PaymentError::Rejected(message))) => {
            assert_eq!(message, "Bad Request: USER_ID_INVALID");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        ledger::status(&services, &stars_event(update.clone()))
            .await
            .unwrap(),
        Some(ledger::Status::Review)
    );
    assert_eq!(
        review.list(PageRequest::default()).await.unwrap().items[0].id,
        id
    );
    let failed = audit_of(&db, REFUND_FAILED_ACTION).await;
    assert_eq!(failed.len(), 2);
    assert_eq!(failed[0].actor_id, Some(admin.0));
    assert_eq!(failed[0].entity_id, Some(payment.id.0));
    assert_eq!(failed[0].details["comment"], "первая попытка");
    assert!(
        failed[1].details["error"]
            .as_str()
            .is_some_and(|e| e.contains("USER_ID_INVALID")),
        "{}",
        failed[1].details
    );
    assert!(!failed[0].details.to_string().contains(TOKEN));
    assert!(audit_of(&db, REFUNDED_ACTION).await.is_empty());
    let done = review
        .refund(id, Some("третья попытка"), admin)
        .await
        .unwrap();
    assert_eq!(done.status, StarsResolutionKind::Refunded);
    assert!(!done.repeated);
    assert_eq!(refund_calls(&server).await.len(), 3);
    assert_eq!(
        ledger::status(&services, &stars_event(update))
            .await
            .unwrap(),
        Some(ledger::Status::Refunded)
    );
    assert_eq!(audit_of(&db, REFUNDED_ACTION).await.len(), 1);
    db.cleanup().await;
}

#[tokio::test]
async fn одновременные_возвраты_одного_события_обращаются_к_telegram_один_раз() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let admin = owner(&services).await;
    let (_payment, _update, id) = under_review(&db, &services, &ctx, 764, "stx-764").await;
    Mock::given(method("POST"))
        .and(path(refund_path()))
        .respond_with(ok(serde_json::json!(true)).set_delay(Duration::from_millis(400)))
        .expect(1)
        .mount(&server)
        .await;
    let telegram = TelegramStars::with_api_base(server.uri());
    let review = StarsReview::with_telegram(&services, &telegram);
    let (first, second, third) = tokio::join!(
        review.refund(id, Some("первый"), admin),
        review.refund(id, Some("второй"), admin),
        review.refund(id, Some("третий"), admin),
    );
    let results = [first.unwrap(), second.unwrap(), third.unwrap()];
    assert!(
        results
            .iter()
            .all(|r| r.status == StarsResolutionKind::Refunded)
    );
    assert_eq!(results.iter().filter(|r| !r.repeated).count(), 1);
    let comment = results
        .iter()
        .find(|r| !r.repeated)
        .and_then(|r| r.comment.clone());
    assert!(results.iter().all(|r| r.comment == comment));
    assert_eq!(refund_calls(&server).await.len(), 1);
    assert_eq!(audit_of(&db, REFUNDED_ACTION).await.len(), 1);
    db.cleanup().await;
}

#[tokio::test]
async fn закрытое_вручную_событие_бот_не_проводит_снова() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let admin = owner(&services).await;
    let (payment, update, id) = under_review(&db, &services, &ctx, 765, "stx-765").await;
    let closed = StarsReview::new(&services)
        .close(id, "Клиенту продлили подписку вручную", admin)
        .await
        .unwrap();
    assert_eq!(closed.status, StarsResolutionKind::Closed);
    assert!(refund_calls(&server).await.is_empty());
    assert_eq!(
        ledger::status(&services, &stars_event(update.clone()))
            .await
            .unwrap(),
        Some(ledger::Status::Closed)
    );
    handle_update(&ctx, serde_json::from_value(update.clone()).unwrap()).await;
    assert_eq!(
        ledger::status(&services, &stars_event(update))
            .await
            .unwrap(),
        Some(ledger::Status::Closed)
    );
    assert_eq!(reviews(&db, payment.id).await, 1);
    assert_eq!(
        payment_of(&db, payment.id).await.status,
        PaymentStatus::Pending
    );
    assert_eq!(
        ledger::retry_due(&services, Duration::from_millis(1))
            .await
            .unwrap(),
        ledger::Retried::default()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn после_возврата_звёзд_от_telegram_клиент_получает_сообщение_один_раз() {
    let Some((db, services, _server, ctx)) = setup().await else {
        return;
    };
    let payment = stars_invoice(&db, &services, 766).await;
    handle_update(
        &ctx,
        serde_json::from_value(paid_update(130, 766, &payment, "stx-766")).unwrap(),
    )
    .await;
    assert_eq!(
        payment_of(&db, payment.id).await.status,
        PaymentStatus::Paid
    );
    assert!(stars_notices(&db).await.is_empty());
    let echo = refund_echo(131, 766, &payment, "stx-766", 350);
    handle_update(&ctx, serde_json::from_value(echo.clone()).unwrap()).await;
    assert_eq!(
        payment_of(&db, payment.id).await.status,
        PaymentStatus::Refunded
    );
    let notices = stars_notices(&db).await;
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].0, payment.client_id);
    assert_eq!(notices[0].1["stars"], 350);
    assert_eq!(notices[0].1["label"], "Стандарт 766 · 90\u{a0}дней");
    assert_eq!(notices[0].1["payment_id"], payment.id.0);
    handle_update(&ctx, serde_json::from_value(echo.clone()).unwrap()).await;
    assert_eq!(
        stars_notices(&db).await.len(),
        1,
        "повтор того же апдейта написал клиенту второй раз"
    );
    let blocked = stars_invoice(&db, &services, 767).await;
    handle_update(
        &ctx,
        serde_json::from_value(paid_update(132, 767, &blocked, "stx-767")).unwrap(),
    )
    .await;
    {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query(
            "INSERT INTO refunds (payment_id, amount_minor, currency, method, subscription_action, reason) \
             VALUES ($1, 350, 'XTR', 'provider', 'keep', $2)",
        )
        .bind(blocked.id.0)
        .bind(sn_services::inbox::BLOCKED_REFUND_REASON)
        .execute(conn.conn())
        .await
        .unwrap();
        conn.payments()
            .transition(blocked.id, PaymentStatus::Refunded, None, None)
            .await
            .unwrap();
    }
    let blocked_echo = refund_echo(133, 767, &blocked, "stx-767", 350);
    handle_update(&ctx, serde_json::from_value(blocked_echo.clone()).unwrap()).await;
    assert_eq!(
        ledger::status(&services, &stars_event(blocked_echo))
            .await
            .unwrap(),
        Some(ledger::Status::Done)
    );
    assert_eq!(
        stars_notices(&db).await.len(),
        1,
        "заблокированному клиенту уже написали о возврате оплаты"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn оплату_звёздами_после_сбоя_можно_провести_снова_без_двойной_выдачи() {
    let Some((db, services, _server, ctx)) = setup().await else {
        return;
    };
    let admin = owner(&services).await;
    let payment = stars_invoice(&db, &services, 770).await;
    let update = paid_update(140, 770, &payment, "stx-770");
    let id = seed_failed(&db, &payment, &update).await;
    let (_mismatch, _, mismatch_id) = under_review(&db, &services, &ctx, 771, "stx-771").await;
    let review = StarsReview::new(&services);
    let page = review.list(PageRequest::default()).await.unwrap();
    let failed = page.items.iter().find(|item| item.id == id).unwrap();
    assert!(failed.retryable, "после сбоя оплату можно провести снова");
    assert!(failed.refundable);
    let mismatch = page
        .items
        .iter()
        .find(|item| item.id == mismatch_id)
        .unwrap();
    assert!(
        !mismatch.retryable,
        "при расхождении суммы провести снова нельзя"
    );
    assert_eq!(
        review.retry(mismatch_id, admin).await.unwrap_err().code(),
        "stars.review_not_retryable"
    );
    assert_eq!(
        ledger::status(&services, &stars_event(update.clone()))
            .await
            .unwrap(),
        Some(ledger::Status::Review)
    );
    let done = review.retry(id, admin).await.unwrap();
    assert_eq!(done.outcome, StarsRetryOutcome::Done);
    assert_eq!(done.id, id);
    assert_eq!(done.charge_id, "stx-770");
    assert_eq!(done.payment_id, Some(payment.id));
    let paid = payment_of(&db, payment.id).await;
    assert_eq!(paid.status, PaymentStatus::Paid);
    assert_eq!(paid.provider_txid.as_deref(), Some("stx-770"));
    assert_eq!(order_status(&db, &paid).await, OrderStatus::Paid);
    assert_eq!(
        ledger::status(&services, &stars_event(update.clone()))
            .await
            .unwrap(),
        Some(ledger::Status::Done)
    );
    assert_eq!(fulfil_jobs(&db).await, 1);
    assert_eq!(
        review.retry(id, admin).await.unwrap_err().code(),
        "stars.review_status"
    );
    handle_update(&ctx, serde_json::from_value(update.clone()).unwrap()).await;
    assert_eq!(
        fulfil_jobs(&db).await,
        1,
        "повтор апдейта выдал заказ второй раз"
    );
    assert_eq!(
        payment_of(&db, payment.id).await.provider_txid.as_deref(),
        Some("stx-770")
    );
    let audit = audit_of(&db, RETRIED_ACTION).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].actor_kind, "admin");
    assert_eq!(audit[0].actor_id, Some(admin.0));
    assert_eq!(audit[0].entity_id, Some(payment.id.0));
    assert_eq!(audit[0].details["charge_id"], "stx-770");
    assert_eq!(audit[0].details["reason"], "модуль не ответил");
    assert!(
        review
            .list(PageRequest::default())
            .await
            .unwrap()
            .items
            .iter()
            .all(|item| item.id != id)
    );
    db.cleanup().await;
}

#[tokio::test]
async fn повторное_проведение_при_новом_сбое_оставляет_оплату_в_очереди() {
    let Some((db, services, _server, _ctx)) = setup().await else {
        return;
    };
    let admin = owner(&services).await;
    let payment = stars_invoice(&db, &services, 772).await;
    let update = paid_update(150, 772, &payment, "stx-772");
    let id = seed_failed(&db, &payment, &update).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        for sql in [
            "CREATE FUNCTION test_refuse_payment() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN RAISE EXCEPTION 'база недоступна' USING ERRCODE = '40001'; END $$",
            "CREATE TRIGGER payments_refuse BEFORE UPDATE ON payments \
             FOR EACH ROW EXECUTE FUNCTION test_refuse_payment()",
        ] {
            sqlx::query(sql).execute(conn.conn()).await.unwrap();
        }
    }
    let review = StarsReview::new(&services);
    let waiting = review.retry(id, admin).await.unwrap();
    assert_eq!(waiting.outcome, StarsRetryOutcome::Pending);
    assert!(waiting.reason.is_some());
    assert_eq!(
        ledger::status(&services, &stars_event(update.clone()))
            .await
            .unwrap(),
        Some(ledger::Status::Pending)
    );
    assert!(
        review
            .list(PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        payment_of(&db, payment.id).await.status,
        PaymentStatus::Pending
    );
    {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query("DROP TRIGGER payments_refuse ON payments")
            .execute(conn.conn())
            .await
            .unwrap();
        sqlx::query("UPDATE bot_star_events SET next_attempt_at = now() - interval '1 second' WHERE id = $1")
            .bind(id)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let retried = ledger::retry_due(&services, Duration::from_millis(1))
        .await
        .unwrap();
    assert_eq!(retried.applied, 1);
    assert_eq!(
        payment_of(&db, payment.id).await.status,
        PaymentStatus::Paid
    );
    assert_eq!(fulfil_jobs(&db).await, 1);
    db.cleanup().await;
}
