use super::*;
use crate::Actor;
use sn_domain::alerts::AlertKind;

const ADMIN: Actor = Actor::Admin(AdminId(1));

async fn kassa_invoice(kit: &Kit, client_id: ClientId, tariff: &Tariff) -> Payment {
    let result = kit
        .services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: cart(tariff, 90, None),
                provider: KASSA.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Redirect { payment_id, .. } = result else {
        panic!("ожидался счёт");
    };
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().by_public_id(payment_id).await.unwrap()
}

async fn block(kit: &Kit, client_id: ClientId) {
    kit.services
        .clients()
        .block(client_id, Some("мошенничество"), ADMIN)
        .await
        .unwrap();
}

async fn paid_after_block(kit: &Kit, telegram_id: i64) -> (ClientId, Payment) {
    let client_id = client(&kit.services, telegram_id).await;
    let tariff = standard(&kit.services).await;
    let invoice = kassa_invoice(kit, client_id, &tariff).await;
    block(kit, client_id).await;
    assert_eq!(
        payment(kit, invoice.id).await.status,
        PaymentStatus::Canceled
    );
    kit.services
        .inbox()
        .apply_outcome(KASSA, &outcome(&invoice, ProviderStatus::Paid))
        .await
        .unwrap();
    assert_eq!(
        order(&kit.services, invoice.order_id).await.status,
        OrderStatus::Paid
    );
    jobs(&kit.services).await;
    (client_id, invoice)
}

fn notify_params(queued: &[JobPayload], template: &str) -> Vec<serde_json::Value> {
    queued
        .iter()
        .filter_map(|job| match job {
            JobPayload::Notify {
                template: name,
                params,
                ..
            } if name == template => Some(params.clone()),
            _ => None,
        })
        .collect()
}

async fn payment_audit(kit: &Kit, payment: PaymentId) -> Vec<sn_store::AuditRecord> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.audit()
        .for_entity("payment", payment.0, 20)
        .await
        .unwrap()
}

#[tokio::test]
async fn поздняя_оплата_заблокированного_клиента_возвращается_автоматически() {
    let kassa = Arc::new(Kassa::default());
    *kassa.refund_completed.lock().unwrap() = true;
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let alerts = kit.services.alerts();
    assert!(alerts.collect().await.unwrap().is_empty());
    let (client_id, invoice) = paid_after_block(&kit, 7700).await;
    let fulfillment = kit.services.fulfillment();
    let refunded = fulfillment.run(invoice.order_id).await.unwrap();
    assert!(
        matches!(&refunded, FulfillmentOutcome::Refunded { .. }),
        "{refunded:?}"
    );
    assert_eq!(
        order(&kit.services, invoice.order_id).await.status,
        OrderStatus::Refunded
    );
    assert_eq!(
        payment(&kit, invoice.id).await.status,
        PaymentStatus::Refunded
    );
    let sent = kassa.refunds.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].amount, invoice.amount);
    assert_eq!(sent[0].reason.as_deref(), Some(BLOCKED_REFUND_REASON));
    let stored = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.refunds().for_payment(invoice.id).await.unwrap()
    };
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].admin_id, None);
    assert_eq!(stored[0].reason.as_deref(), Some(BLOCKED_REFUND_REASON));
    assert_eq!(
        stored[0].subscription_action,
        RefundSubscriptionAction::Keep
    );
    let audit = payment_audit(&kit, invoice.id).await;
    let journal = audit
        .iter()
        .find(|r| r.action == BLOCKED_REFUND_AUDIT)
        .expect("в журнале нет автоматического возврата");
    assert_eq!(journal.actor_kind, "system");
    assert_eq!(journal.actor_id, None);
    assert_eq!(
        journal.details["order_id"],
        serde_json::json!(invoice.order_id)
    );
    assert!(!audit.iter().any(|r| r.action == "payment.refund"));
    let queued = jobs(&kit.services).await;
    assert_eq!(
        notifications(&queued, client_id),
        vec![BLOCKED_REFUND_TEMPLATE.to_string()]
    );
    let letter = notify_params(&queued, BLOCKED_REFUND_TEMPLATE);
    assert_eq!(letter[0]["amount"], "$6.99");
    assert!(kit.remna.users().is_empty());
    let again = fulfillment.run(invoice.order_id).await.unwrap();
    assert!(
        matches!(again, FulfillmentOutcome::Review { .. }),
        "{again:?}"
    );
    assert_eq!(kassa.refund_keys.lock().unwrap().len(), 1);
    let got = alerts.collect().await.unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].kind, AlertKind::Refunds);
    assert!(
        got[0].html.contains("Оплату вернули автоматически"),
        "{}",
        got[0].html
    );
    assert!(got[0].html.contains(&format!(
        "Клиент заблокирован, заказ №{} не выдан",
        invoice.order_id
    )));
    assert!(got[0].html.contains("Сумма: $6.99"), "{}", got[0].html);
    assert!(got[0].html.contains("Клиент: Алексей (@user7700)"));
    assert!(!got[0].html.contains("Ждём подтверждения"));
    assert!(alerts.collect().await.unwrap().is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn возврат_заблокированному_ждёт_подтверждения_кассы_и_клиенту_пишем_после_него() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let alerts = kit.services.alerts();
    alerts.collect().await.unwrap();
    let (client_id, invoice) = paid_after_block(&kit, 7701).await;
    let fulfillment = kit.services.fulfillment();
    for _ in 0..2 {
        let waiting = fulfillment.run(invoice.order_id).await.unwrap();
        assert!(
            matches!(&waiting, FulfillmentOutcome::Retry { reason, .. } if reason.contains("возврат")),
            "{waiting:?}"
        );
    }
    assert_eq!(kassa.refunds.lock().unwrap().len(), 1);
    assert_eq!(
        order(&kit.services, invoice.order_id).await.status,
        OrderStatus::Paid
    );
    let requests = provider_refunds(&kit, invoice.id).await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].status, ProviderRefundStatus::Pending);
    assert_eq!(requests[0].admin_id, None);
    assert_eq!(requests[0].reason.as_deref(), Some(BLOCKED_REFUND_REASON));
    assert_eq!(
        requests[0].subscription_action,
        RefundSubscriptionAction::Keep
    );
    assert!(
        audit_actions(&kit, invoice.id)
            .await
            .contains(&BLOCKED_REFUND_REQUESTED_AUDIT.to_string())
    );
    let queued = jobs(&kit.services).await;
    assert!(notifications(&queued, client_id).is_empty());
    let got = alerts.collect().await.unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].html.contains("Оплату возвращаем автоматически"));
    assert!(got[0].html.contains("Ждём подтверждения платёжной системы"));
    let confirmed = WebhookOutcome {
        event_key: "refund:kr-1".into(),
        ..outcome(&invoice, ProviderStatus::Refunded)
    };
    assert_eq!(
        kit.services
            .inbox()
            .apply_outcome(KASSA, &confirmed)
            .await
            .unwrap(),
        AcceptResult::StatusChanged {
            payment_id: invoice.id,
            status: PaymentStatus::Refunded
        }
    );
    assert_eq!(
        order(&kit.services, invoice.order_id).await.status,
        OrderStatus::Refunded
    );
    let queued = jobs(&kit.services).await;
    assert_eq!(
        notifications(&queued, client_id),
        vec![BLOCKED_REFUND_TEMPLATE.to_string()]
    );
    assert_eq!(
        notify_params(&queued, BLOCKED_REFUND_TEMPLATE)[0]["amount"],
        "$6.99"
    );
    assert!(alerts.collect().await.unwrap().is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn сбой_кассы_при_автоматическом_возврате_повторяется_с_тем_же_ключом() {
    let kassa = Arc::new(Kassa::default());
    *kassa.refund_completed.lock().unwrap() = true;
    *kassa.refund_fails.lock().unwrap() = true;
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let (_, invoice) = paid_after_block(&kit, 7702).await;
    let fulfillment = kit.services.fulfillment();
    let first = fulfillment.run(invoice.order_id).await.unwrap();
    assert!(
        matches!(&first, FulfillmentOutcome::Retry { reason, .. } if reason.contains("касса не отвечает")),
        "{first:?}"
    );
    assert_eq!(
        order(&kit.services, invoice.order_id).await.status,
        OrderStatus::Paid
    );
    assert!(provider_refunds(&kit, invoice.id).await.is_empty());
    *kassa.refund_fails.lock().unwrap() = false;
    let second = fulfillment.run(invoice.order_id).await.unwrap();
    assert!(
        matches!(second, FulfillmentOutcome::Refunded { .. }),
        "{second:?}"
    );
    let keys = kassa.refund_keys.lock().unwrap().clone();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], keys[1]);
    assert_eq!(kassa.refunds.lock().unwrap().len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn после_разблокировки_оплату_автоматически_не_возвращаем() {
    let kassa = Arc::new(Kassa::default());
    *kassa.refund_completed.lock().unwrap() = true;
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let (client_id, invoice) = paid_after_block(&kit, 7703).await;
    kit.services
        .clients()
        .unblock(client_id, ADMIN)
        .await
        .unwrap();
    assert_eq!(
        kit.services
            .inbox()
            .refund_blocked(invoice.order_id)
            .await
            .unwrap(),
        BlockedRefund::Unblocked
    );
    assert!(kassa.refund_keys.lock().unwrap().is_empty());
    assert_eq!(payment(&kit, invoice.id).await.status, PaymentStatus::Paid);
    assert_eq!(
        order(&kit.services, invoice.order_id).await.status,
        OrderStatus::Paid
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn оплата_до_блокировки_и_оплата_с_баланса_ждут_админа() {
    let kassa = Arc::new(Kassa::default());
    *kassa.refund_completed.lock().unwrap() = true;
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let client_id = client(&kit.services, 7704).await;
    let tariff = standard(&kit.services).await;
    let invoice = kassa_invoice(&kit, client_id, &tariff).await;
    kit.services
        .inbox()
        .apply_outcome(KASSA, &outcome(&invoice, ProviderStatus::Paid))
        .await
        .unwrap();
    credit(&kit.services, client_id, 1_000).await;
    let from_balance = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    block(&kit, client_id).await;
    let fulfillment = kit.services.fulfillment();
    for (order_id, why) in [
        (invoice.order_id, "оплата пришла до блокировки"),
        (from_balance.id, "оплата прошла с баланса"),
    ] {
        let held = fulfillment.run(order_id).await.unwrap();
        assert!(
            matches!(&held, FulfillmentOutcome::Review { reason }
                if reason.contains("клиент заблокирован")
                    && reason.contains(&format!("Оплату не вернули автоматически: {why}"))),
            "{held:?}"
        );
        assert_eq!(
            order(&kit.services, order_id).await.status,
            OrderStatus::Review
        );
    }
    assert!(kassa.refund_keys.lock().unwrap().is_empty());
    assert_eq!(payment(&kit, invoice.id).await.status, PaymentStatus::Paid);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn после_отказа_кассы_в_автоматическом_возврате_заказ_ждёт_админа() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let (client_id, invoice) = paid_after_block(&kit, 7705).await;
    let fulfillment = kit.services.fulfillment();
    let waiting = fulfillment.run(invoice.order_id).await.unwrap();
    assert!(
        matches!(waiting, FulfillmentOutcome::Retry { .. }),
        "{waiting:?}"
    );
    let declined = WebhookOutcome {
        event_key: "refund:kr-1".into(),
        ..outcome(&invoice, ProviderStatus::RefundDeclined)
    };
    kit.services
        .inbox()
        .apply_outcome(KASSA, &declined)
        .await
        .unwrap();
    assert_eq!(
        provider_refunds(&kit, invoice.id).await[0].status,
        ProviderRefundStatus::Declined
    );
    let held = fulfillment.run(invoice.order_id).await.unwrap();
    assert!(
        matches!(&held, FulfillmentOutcome::Review { reason }
            if reason.contains("клиент заблокирован")
                && reason.contains("заявку на возврат закрыли без возврата: платёжная система отклонила возврат")),
        "{held:?}"
    );
    assert_eq!(
        order(&kit.services, invoice.order_id).await.status,
        OrderStatus::Review
    );
    assert_eq!(kassa.refund_keys.lock().unwrap().len(), 1);
    assert_eq!(payment(&kit, invoice.id).await.status, PaymentStatus::Paid);
    assert!(notifications(&jobs(&kit.services).await, client_id).is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn разблокировка_после_потерянного_ответа_кассы_не_выдаёт_заказ_сама() {
    let kassa = Arc::new(Kassa::default());
    *kassa.refund_completed.lock().unwrap() = true;
    *kassa.refund_lost.lock().unwrap() = true;
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let (client_id, invoice) = paid_after_block(&kit, 7710).await;
    let fulfillment = kit.services.fulfillment();
    let first = fulfillment.run(invoice.order_id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    assert_eq!(kassa.refunds.lock().unwrap().len(), 1);
    kit.services
        .clients()
        .unblock(client_id, ADMIN)
        .await
        .unwrap();
    *kassa.refund_lost.lock().unwrap() = false;
    let second = fulfillment.run(invoice.order_id).await.unwrap();
    assert!(
        matches!(&second, FulfillmentOutcome::Review { reason } if reason.contains("касса не ответила")),
        "{second:?}"
    );
    assert_eq!(kassa.refunds.lock().unwrap().len(), 1);
    assert_eq!(
        order(&kit.services, invoice.order_id).await.status,
        OrderStatus::Review
    );
    fulfillment
        .redeliver(invoice.order_id, ADMIN)
        .await
        .unwrap();
    let third = fulfillment.run(invoice.order_id).await.unwrap();
    assert!(
        matches!(third, FulfillmentOutcome::Done { .. }),
        "{third:?}"
    );
    kit.db.cleanup().await;
}
