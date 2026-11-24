use super::*;
use crate::autorenew::tests::{
    FlakyPay, flaky_kit, notifications, payment_audit, renewals, unknown_charge,
};
use crate::autorenew::{CHARGE_BLOCKED_AUDIT, CHARGE_UNCLEAR_AUDIT, ChargeCheck};
use crate::checkout::testkit::*;
use crate::checkout::{Channel, CheckoutRequest, CheckoutResult};
use crate::fulfillment::FulfillmentOutcome;
use sn_domain::order::Order;
use sn_domain::payment::Payment;
use sn_payments::{PaymentError, PaymentRef, ProviderStatus, WebhookOutcome, WebhookReply};

const ADMIN: Actor = Actor::Admin(AdminId(1));

async fn payment(kit: &Kit, id: PaymentId) -> Payment {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().get(id).await.unwrap()
}

async fn open_invoice(kit: &Kit, subscription: &Subscription) -> (Order, Payment) {
    let tariff = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.tariffs()
            .get(subscription.tariff_id.unwrap())
            .await
            .unwrap()
    };
    let result = kit
        .services
        .checkout()
        .start(
            subscription.client_id,
            &CheckoutRequest {
                cart: cart(&tariff, 30, None),
                provider: TEST_PAY.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect {
        order_id,
        payment_id,
        ..
    } = result
    else {
        panic!("ожидался счёт: {result:?}");
    };
    let order = order_by_public_id(&kit.services, order_id).await;
    let mut conn = kit.services.store().conn().await.unwrap();
    let payment = conn.payments().by_public_id(payment_id).await.unwrap();
    (order, payment)
}

async fn block_audit(kit: &Kit, client_id: ClientId) -> sn_store::AuditRecord {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.audit()
        .for_entity("client", client_id.0, 20)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.action == "client.block")
        .unwrap()
}

async fn poll_job_status(kit: &Kit, payment: &Payment) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM jobs WHERE dedupe_key = $1 ORDER BY id DESC LIMIT 1")
        .bind(format!("poll:{}", payment.id))
        .fetch_optional(kit.services.store().pool())
        .await
        .unwrap()
}

fn status_answer(charge: &Payment, status: ProviderStatus) -> WebhookOutcome {
    let txid = charge.provider_txid.clone().unwrap();
    WebhookOutcome {
        event_key: format!("status:{txid}"),
        payment: None,
        provider_txid: Some(txid),
        status,
        amount: Some(charge.amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    }
}

async fn inflight_charge(kit: &Kit, telegram_id: i64) -> (Subscription, Payment) {
    *kit.pay.charge.lock().unwrap() = Some(ProviderStatus::Pending);
    let (subscription, charge) = unknown_charge(kit, telegram_id).await;
    assert!(charge.provider_txid.is_some());
    (subscription, charge)
}

#[tokio::test]
async fn блокировка_не_отменяет_автосписание_а_подтверждённая_оплата_возвращается() {
    let Some(kit) = kit().await else { return };
    let (subscription, charge) = inflight_charge(&kit, 401).await;
    let client_id = subscription.client_id;
    let (opened, bill) = open_invoice(&kit, &subscription).await;
    kit.services
        .clients()
        .block(client_id, Some("мошенничество"), ADMIN)
        .await
        .unwrap();
    assert_eq!(
        payment(&kit, bill.id).await.status,
        PaymentStatus::Canceled,
        "обычный счёт отменён"
    );
    assert_eq!(
        order(&kit.services, opened.id).await.status,
        OrderStatus::Canceled
    );
    assert_eq!(
        payment(&kit, charge.id).await.status,
        PaymentStatus::Pending,
        "автосписание с невыясненным исходом не отменено"
    );
    assert_eq!(
        order(&kit.services, charge.order_id).await.status,
        OrderStatus::Pending
    );
    assert_eq!(
        poll_job_status(&kit, &charge).await.as_deref(),
        Some("queued")
    );
    let audit = block_audit(&kit, client_id).await;
    assert_eq!(
        audit.details["canceled_payments"],
        serde_json::json!([bill.id])
    );
    assert_eq!(
        audit.details["canceled_orders"],
        serde_json::json!([opened.id])
    );
    assert_eq!(
        audit.details["kept_autorenew_payments"],
        serde_json::json!([charge.id])
    );
    *kit.pay.status.lock().unwrap() = Some(status_answer(&charge, ProviderStatus::Paid));
    let autorenew = kit.services.autorenew();
    assert_eq!(
        autorenew.check_charge(charge.id).await.unwrap(),
        Some(ChargeCheck::Settled)
    );
    assert_eq!(payment(&kit, charge.id).await.status, PaymentStatus::Paid);
    let outcome = kit
        .services
        .fulfillment()
        .run(charge.order_id)
        .await
        .unwrap();
    assert!(
        matches!(&outcome, FulfillmentOutcome::Refunded { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        order(&kit.services, charge.order_id).await.status,
        OrderStatus::Refunded
    );
    assert_eq!(
        payment(&kit, charge.id).await.status,
        PaymentStatus::Refunded
    );
    let sent = kit.pay.refunds.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].amount, charge.amount);
    assert_eq!(
        autorenew.check_charge(charge.id).await.unwrap(),
        Some(ChargeCheck::Settled)
    );
    let flagged = payment_audit(&kit, &charge, CHARGE_BLOCKED_AUDIT).await;
    assert_eq!(flagged.len(), 1, "{flagged:?}");
    assert_eq!(flagged[0].actor_kind, "system");
    assert_eq!(
        flagged[0].details["order_id"],
        serde_json::json!(charge.order_id)
    );
    assert_eq!(
        flagged[0].details["subscription_id"],
        serde_json::json!(subscription.id)
    );
    assert_eq!(
        flagged[0].details["client_id"],
        serde_json::json!(client_id)
    );
    assert!(kit.remna.calls_of("update_user").is_empty());
    assert_eq!(kit.pay.charges.lock().unwrap().len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отказ_кассы_после_блокировки_отменяет_автосписание_без_уведомления_клиенту() {
    let Some(kit) = kit().await else { return };
    let (subscription, charge) = inflight_charge(&kit, 402).await;
    kit.services
        .clients()
        .block(subscription.client_id, Some("мошенничество"), ADMIN)
        .await
        .unwrap();
    assert_eq!(
        payment(&kit, charge.id).await.status,
        PaymentStatus::Pending
    );
    jobs(&kit.services).await;
    *kit.pay.status.lock().unwrap() = Some(status_answer(&charge, ProviderStatus::Canceled));
    assert_eq!(
        kit.services
            .autorenew()
            .check_charge(charge.id)
            .await
            .unwrap(),
        Some(ChargeCheck::Settled)
    );
    assert_eq!(
        payment(&kit, charge.id).await.status,
        PaymentStatus::Canceled
    );
    assert_eq!(
        order(&kit.services, charge.order_id).await.status,
        OrderStatus::Canceled
    );
    let reason: Option<String> =
        sqlx::query_scalar("SELECT failure_reason FROM payments WHERE id = $1")
            .bind(charge.id.0)
            .fetch_one(kit.services.store().pool())
            .await
            .unwrap();
    assert_ne!(reason.as_deref(), Some(BLOCKED_REASON));
    let queued = jobs(&kit.services).await;
    assert_eq!(notifications(&queued, "autorenew.failed"), 0);
    assert!(
        payment_audit(&kit, &charge, CHARGE_BLOCKED_AUDIT)
            .await
            .is_empty()
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn после_блокировки_автосписание_без_номера_платежа_ждёт_ручной_проверки() {
    let pay = FlakyPay::answering(vec![
        Err(PaymentError::Transport("обрыв связи".into())),
        Ok(ProviderStatus::Paid),
    ]);
    let Some(kit) = flaky_kit(pay.clone()).await else {
        return;
    };
    let (subscription, charge) = unknown_charge(&kit, 403).await;
    assert_eq!(charge.provider_txid, None);
    kit.services
        .clients()
        .block(subscription.client_id, Some("мошенничество"), ADMIN)
        .await
        .unwrap();
    assert_eq!(
        payment(&kit, charge.id).await.status,
        PaymentStatus::Pending
    );
    let checked = kit
        .services
        .autorenew()
        .check_charge(charge.id)
        .await
        .unwrap();
    assert!(
        matches!(&checked, Some(ChargeCheck::Unclear(reason)) if reason.contains("заблокирован")),
        "{checked:?}"
    );
    assert_eq!(pay.requests().len(), 1);
    assert_eq!(
        payment(&kit, charge.id).await.status,
        PaymentStatus::Pending
    );
    let renewals = renewals(&kit, &subscription).await;
    assert_eq!(renewals[0].0.status, OrderStatus::Pending);
    let audit = payment_audit(&kit, &charge, CHARGE_UNCLEAR_AUDIT).await;
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert!(
        audit[0].details["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("заблокирован")),
        "{audit:?}"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn разблокированный_клиент_получает_продление_по_подтверждённому_автосписанию() {
    let Some(kit) = kit().await else { return };
    let (subscription, charge) = inflight_charge(&kit, 404).await;
    let client_id = subscription.client_id;
    let expire = subscription.mirror.expire_at.unwrap();
    kit.services
        .clients()
        .block(client_id, Some("проверка"), ADMIN)
        .await
        .unwrap();
    kit.services
        .clients()
        .unblock(client_id, ADMIN)
        .await
        .unwrap();
    assert_eq!(
        payment(&kit, charge.id).await.status,
        PaymentStatus::Pending
    );
    *kit.pay.status.lock().unwrap() = Some(status_answer(&charge, ProviderStatus::Paid));
    assert_eq!(
        kit.services
            .autorenew()
            .check_charge(charge.id)
            .await
            .unwrap(),
        Some(ChargeCheck::Settled)
    );
    let outcome = kit
        .services
        .fulfillment()
        .run(charge.order_id)
        .await
        .unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        remna_user(&kit, &subscription).await.expire_at,
        expire + chrono::Duration::days(90)
    );
    assert!(
        payment_audit(&kit, &charge, CHARGE_BLOCKED_AUDIT)
            .await
            .is_empty()
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn автосписание_подтверждённое_уведомлением_кассы_после_блокировки_тоже_попадает_в_аудит() {
    let Some(kit) = kit().await else { return };
    let (subscription, charge) = inflight_charge(&kit, 405).await;
    kit.services
        .clients()
        .block(subscription.client_id, Some("мошенничество"), ADMIN)
        .await
        .unwrap();
    let notice = WebhookOutcome {
        payment: Some(PaymentRef::Payment(charge.id)),
        ..status_answer(&charge, ProviderStatus::Paid)
    };
    kit.services
        .inbox()
        .apply_outcome(TEST_PAY, &notice)
        .await
        .unwrap();
    assert_eq!(payment(&kit, charge.id).await.status, PaymentStatus::Paid);
    assert!(
        payment_audit(&kit, &charge, CHARGE_BLOCKED_AUDIT)
            .await
            .is_empty()
    );
    for _ in 0..2 {
        assert_eq!(
            kit.services
                .autorenew()
                .check_charge(charge.id)
                .await
                .unwrap(),
            Some(ChargeCheck::Settled)
        );
    }
    assert_eq!(
        payment_audit(&kit, &charge, CHARGE_BLOCKED_AUDIT)
            .await
            .len(),
        1
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn после_разблокировки_второе_списание_не_уходит_пока_исход_первого_не_выяснен() {
    let Some(kit) = kit().await else { return };
    let (subscription, charge) = inflight_charge(&kit, 406).await;
    let client_id = subscription.client_id;
    kit.services
        .clients()
        .block(client_id, Some("проверка"), ADMIN)
        .await
        .unwrap();
    kit.services
        .clients()
        .unblock(client_id, ADMIN)
        .await
        .unwrap();
    let current = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.subscriptions().get(subscription.id).await.unwrap()
    };
    assert!(current.autorenew.enabled);
    let next_day = kit.services.now() + chrono::Duration::days(1);
    let report = kit.services.autorenew().run_due(next_day).await.unwrap();
    assert_eq!((report.due, report.charged, report.failed), (1, 0, 0));
    assert_eq!(
        kit.pay.charges.lock().unwrap().len(),
        1,
        "второго запроса на списание нет"
    );
    assert_eq!(
        payment(&kit, charge.id).await.status,
        PaymentStatus::Pending
    );
    assert_eq!(renewals(&kit, &subscription).await.len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn после_разблокировки_опрос_автосписания_без_номера_платежа_возобновляется() {
    let pay = FlakyPay::answering(vec![
        Err(PaymentError::Transport("обрыв связи".into())),
        Ok(ProviderStatus::Paid),
    ]);
    let Some(kit) = flaky_kit(pay.clone()).await else {
        return;
    };
    let (subscription, charge) = unknown_charge(&kit, 407).await;
    let client_id = subscription.client_id;
    kit.services
        .clients()
        .block(client_id, Some("проверка"), ADMIN)
        .await
        .unwrap();
    let autorenew = kit.services.autorenew();
    let checked = autorenew.check_charge(charge.id).await.unwrap();
    assert!(
        matches!(&checked, Some(ChargeCheck::Unclear(reason)) if reason.contains("заблокирован")),
        "{checked:?}"
    );
    sqlx::query("UPDATE jobs SET status = 'review', attempts = 3 WHERE dedupe_key = $1")
        .bind(format!("poll:{}", charge.id))
        .execute(kit.services.store().pool())
        .await
        .unwrap();
    kit.services
        .clients()
        .unblock(client_id, ADMIN)
        .await
        .unwrap();
    assert_eq!(
        poll_job_status(&kit, &charge).await.as_deref(),
        Some("queued"),
        "опрос автосписания снова в очереди"
    );
    let attempts: i32 = sqlx::query_scalar("SELECT attempts FROM jobs WHERE dedupe_key = $1")
        .bind(format!("poll:{}", charge.id))
        .fetch_one(kit.services.store().pool())
        .await
        .unwrap();
    assert_eq!(attempts, 0);
    assert_eq!(
        autorenew.check_charge(charge.id).await.unwrap(),
        Some(ChargeCheck::Settled)
    );
    assert_eq!(pay.requests().len(), 2);
    assert_eq!(
        pay.requests()[0].payment_public_id,
        pay.requests()[1].payment_public_id
    );
    assert_eq!(payment(&kit, charge.id).await.status, PaymentStatus::Paid);
    assert_eq!(
        order(&kit.services, charge.order_id).await.status,
        OrderStatus::Paid
    );
    kit.db.cleanup().await;
}

async fn charges_in_review(kit: &Kit) -> i64 {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().charges_in_review().await.unwrap()
}

fn charge_in_history(
    page: &HistoryPage,
    charge: &Payment,
) -> Option<(PaymentStatus, Option<String>)> {
    page.items.iter().find_map(|item| match &item.record {
        HistoryRecord::Payment {
            payment, review, ..
        } if payment.number == charge.id => Some((payment.status, review.clone())),
        _ => None,
    })
}

#[tokio::test]
async fn списание_на_ручной_проверке_видно_в_истории_клиента_и_на_главной() {
    let pay = FlakyPay::answering(vec![
        Err(PaymentError::Transport("обрыв связи".into())),
        Ok(ProviderStatus::Paid),
    ]);
    let Some(kit) = flaky_kit(pay.clone()).await else {
        return;
    };
    let (subscription, charge) = unknown_charge(&kit, 408).await;
    let client_id = subscription.client_id;
    assert_eq!(charges_in_review(&kit).await, 0);
    let clients = kit.services.clients();
    let history = clients
        .history(client_id, PageRequest::default(), true)
        .await
        .unwrap();
    assert_eq!(
        charge_in_history(&history, &charge),
        None,
        "списание в пути в истории не показывается"
    );
    clients
        .block(client_id, Some("проверка"), ADMIN)
        .await
        .unwrap();
    let checked = kit
        .services
        .autorenew()
        .check_charge(charge.id)
        .await
        .unwrap();
    let Some(ChargeCheck::Unclear(reason)) = checked else {
        panic!("ожидалась ручная проверка: {checked:?}");
    };
    sqlx::query("UPDATE jobs SET status = 'review', last_error = $2 WHERE dedupe_key = $1")
        .bind(format!("poll:{}", charge.id))
        .bind(&reason)
        .execute(kit.services.store().pool())
        .await
        .unwrap();
    assert_eq!(charges_in_review(&kit).await, 1);
    assert_eq!(
        kit.services
            .admin()
            .dashboard(crate::admin::Sees::ALL)
            .await
            .unwrap()
            .charges_review,
        Some(1)
    );
    let history = clients
        .history(client_id, PageRequest::default(), true)
        .await
        .unwrap();
    assert_eq!(
        charge_in_history(&history, &charge),
        Some((PaymentStatus::Pending, Some(reason.clone())))
    );
    let shown = serde_json::to_value(&history).unwrap();
    let line = shown["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "payment")
        .unwrap();
    assert_eq!(line["review"], reason.as_str());
    assert_eq!(line["payment"]["status"], "pending");
    let without_money = clients
        .history(client_id, PageRequest::default(), false)
        .await
        .unwrap();
    assert_eq!(charge_in_history(&without_money, &charge), None);
    clients.unblock(client_id, ADMIN).await.unwrap();
    assert_eq!(
        charges_in_review(&kit).await,
        0,
        "опрос снова идёт: ручная проверка не нужна"
    );
    let history = clients
        .history(client_id, PageRequest::default(), true)
        .await
        .unwrap();
    assert_eq!(charge_in_history(&history, &charge), None);
    assert_eq!(
        kit.services
            .autorenew()
            .check_charge(charge.id)
            .await
            .unwrap(),
        Some(ChargeCheck::Settled)
    );
    let history = clients
        .history(client_id, PageRequest::default(), true)
        .await
        .unwrap();
    assert_eq!(
        charge_in_history(&history, &charge),
        Some((PaymentStatus::Paid, None))
    );
    kit.db.cleanup().await;
}
