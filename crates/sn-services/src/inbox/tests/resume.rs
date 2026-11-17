use super::*;
use sn_domain::order::Order;

async fn paid_not_delivered(kit: &Kit, telegram_id: i64) -> (Payment, Order) {
    let client_id = client(&kit.services, telegram_id).await;
    let tariff = standard(&kit.services).await;
    let result = kit
        .services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: cart(&tariff, 90, None),
                provider: KASSA.into(),
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect { payment_id, .. } = result else {
        panic!("ожидался счёт");
    };
    let invoice = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments().by_public_id(payment_id).await.unwrap()
    };
    kit.services
        .inbox()
        .apply_outcome(KASSA, &outcome(&invoice, ProviderStatus::Paid))
        .await
        .unwrap();
    let bought = order(&kit.services, invoice.order_id).await;
    assert_eq!(bought.status, OrderStatus::Paid);
    (payment(kit, invoice.id).await, bought)
}

fn fulfill_key(order_id: OrderId) -> String {
    JobPayload::FulfillOrder { order_id }.dedupe_key().unwrap()
}

async fn fulfill_job(kit: &Kit, order_id: OrderId) -> (String, i32) {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT status, attempts FROM jobs WHERE dedupe_key = $1 ORDER BY id DESC LIMIT 1",
    )
    .bind(fulfill_key(order_id))
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

async fn wait_out_attempts(kit: &Kit, order_id: OrderId) {
    let waited = kit.services.fulfillment().run(order_id).await.unwrap();
    assert!(
        matches!(waited, FulfillmentOutcome::Retry { .. }),
        "{waited:?}"
    );
    let mut conn = kit.services.store().conn().await.unwrap();
    let changed = sqlx::query(
        "UPDATE jobs SET status = 'review', attempts = max_attempts, \
         last_error = 'ждём решения платёжной системы о возврате' WHERE dedupe_key = $1",
    )
    .bind(fulfill_key(order_id))
    .execute(conn.conn())
    .await
    .unwrap();
    assert_eq!(changed.rows_affected(), 1);
}

async fn request(kit: &Kit, admin: AdminId, paid: &Payment, minor: Option<i64>) -> i64 {
    let requested = kit
        .services
        .inbox()
        .refund(
            paid.id,
            &refund_command(minor, RefundSubscriptionAction::Keep, Some("Передумал")),
            admin,
        )
        .await
        .unwrap();
    assert!(requested.pending);
    requested.request_id.unwrap()
}

fn declined(paid: &Payment, refund: &str, minor: i64) -> WebhookOutcome {
    WebhookOutcome {
        event_key: format!("refund:{refund}"),
        amount: Some(Amount::new(minor, PayCurrency::Usd)),
        ..outcome(paid, ProviderStatus::RefundDeclined)
    }
}

async fn delivered(kit: &Kit, order_id: OrderId) {
    let done = kit.services.fulfillment().run(order_id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(
        order(&kit.services, order_id).await.status,
        OrderStatus::Fulfilled
    );
}

#[tokio::test]
async fn отказ_кассы_в_возврате_сразу_возвращает_заказ_в_очередь_выдачи() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let admin = admin(&kit).await;
    let (paid, bought) = paid_not_delivered(&kit, 7901).await;
    request(&kit, admin, &paid, None).await;
    wait_out_attempts(&kit, bought.id).await;
    assert_eq!(fulfill_job(&kit, bought.id).await.0, "review");
    kit.services
        .inbox()
        .apply_outcome(KASSA, &declined(&paid, "kr-1", paid.amount.minor))
        .await
        .unwrap();
    assert_eq!(fulfill_job(&kit, bought.id).await, ("queued".into(), 0));
    delivered(&kit, bought.id).await;
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ручная_отмена_заявки_на_возврат_возвращает_заказ_в_очередь_выдачи() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let admin = admin(&kit).await;
    let (paid, bought) = paid_not_delivered(&kit, 7902).await;
    let request_id = request(&kit, admin, &paid, None).await;
    wait_out_attempts(&kit, bought.id).await;
    kit.services
        .inbox()
        .cancel_refund_request(request_id, admin, "касса не ответила за неделю")
        .await
        .unwrap();
    assert_eq!(fulfill_job(&kit, bought.id).await, ("queued".into(), 0));
    delivered(&kit, bought.id).await;
    kit.db.cleanup().await;
}

#[tokio::test]
async fn пока_ждём_второй_возврат_по_заказу_выдача_в_очередь_не_возвращается() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let admin = admin(&kit).await;
    let (paid, bought) = paid_not_delivered(&kit, 7903).await;
    request(&kit, admin, &paid, Some(100)).await;
    request(&kit, admin, &paid, Some(150)).await;
    wait_out_attempts(&kit, bought.id).await;
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(KASSA, &declined(&paid, "kr-1", 100))
        .await
        .unwrap();
    assert_eq!(fulfill_job(&kit, bought.id).await.0, "review");
    let still = kit.services.fulfillment().run(bought.id).await.unwrap();
    assert!(
        matches!(still, FulfillmentOutcome::Retry { .. }),
        "{still:?}"
    );
    inbox
        .apply_outcome(KASSA, &declined(&paid, "kr-2", 150))
        .await
        .unwrap();
    assert_eq!(fulfill_job(&kit, bought.id).await, ("queued".into(), 0));
    delivered(&kit, bought.id).await;
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отказ_в_возврате_выданного_заказа_задачу_выдачи_не_ставит() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let admin = admin(&kit).await;
    let (paid, bought) = paid_not_delivered(&kit, 7904).await;
    delivered(&kit, bought.id).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query("UPDATE jobs SET status = 'done', done_at = now() WHERE dedupe_key = $1")
            .bind(fulfill_key(bought.id))
            .execute(conn.conn())
            .await
            .unwrap();
    }
    request(&kit, admin, &paid, None).await;
    kit.services
        .inbox()
        .apply_outcome(KASSA, &declined(&paid, "kr-1", paid.amount.minor))
        .await
        .unwrap();
    assert_eq!(fulfill_job(&kit, bought.id).await.0, "done");
    kit.db.cleanup().await;
}
