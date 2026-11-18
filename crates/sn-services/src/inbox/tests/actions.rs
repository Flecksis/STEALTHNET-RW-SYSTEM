use super::*;
use crate::FixedClock;
use crate::inbox::refund_action::{cut_target, retry_delay};
use sn_domain::alerts::AlertKind;
use sn_domain::order::Order;
use sn_domain::subscription::Subscription;
use sn_remnawave::UserStatus;
use sn_remnawave::fake::Fault;
use sn_store::{RefundAction, RefundActionStatus};

struct Bought {
    payment: Payment,
    order: Order,
    subscription: Subscription,
}

async fn clocked_kit(kassa: Arc<Kassa>) -> Option<(Kit, Arc<FixedClock>)> {
    let clock = Arc::new(FixedClock::new(moment(chrono::Duration::zero())));
    let kit = kassa_kit_with_clock(kassa, clock.clone()).await?;
    Some((kit, clock))
}

fn advance(kit: &Kit, clock: &FixedClock, by: chrono::Duration) {
    clock.set(kit.services.now() + by);
}

async fn buy(
    kit: &Kit,
    client_id: ClientId,
    tariff: &Tariff,
    days: u32,
    current: Option<&Subscription>,
) -> Bought {
    let result = kit
        .services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: cart(tariff, days, current),
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
    let payment = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments().by_public_id(payment_id).await.unwrap()
    };
    kit.services
        .inbox()
        .apply_outcome(KASSA, &outcome(&payment, ProviderStatus::Paid))
        .await
        .unwrap();
    let done = kit
        .services
        .fulfillment()
        .run(payment.order_id)
        .await
        .unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    let order = order(&kit.services, payment.order_id).await;
    let subscription = subscription(&kit.services, &order).await;
    Bought {
        payment,
        order,
        subscription,
    }
}

async fn request_refund(
    kit: &Kit,
    admin: AdminId,
    bought: &Bought,
    action: RefundSubscriptionAction,
) -> i64 {
    let requested = kit
        .services
        .inbox()
        .refund(
            bought.payment.id,
            &refund_command(None, action, Some("Не подошёл")),
            admin,
        )
        .await
        .unwrap();
    assert!(requested.pending);
    requested.request_id.unwrap()
}

async fn confirm(kit: &Kit, bought: &Bought, refund: &str) {
    let confirmed = WebhookOutcome {
        event_key: format!("refund:{refund}"),
        ..outcome(&bought.payment, ProviderStatus::Refunded)
    };
    assert_eq!(
        kit.services
            .inbox()
            .apply_outcome(KASSA, &confirmed)
            .await
            .unwrap(),
        AcceptResult::StatusChanged {
            payment_id: bought.payment.id,
            status: PaymentStatus::Refunded
        }
    );
}

async fn subscription_audit(kit: &Kit, subscription: &Subscription) -> Vec<String> {
    let mut conn = kit.services.store().conn().await.unwrap();
    let mut actions: Vec<String> = conn
        .audit()
        .for_entity("subscription", subscription.id.0, 50)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.action)
        .collect();
    actions.reverse();
    actions
}

async fn action(kit: &Kit, bought: &Bought) -> RefundAction {
    provider_refunds(kit, bought.payment.id).await[0]
        .action
        .clone()
}

fn failed_audits(actions: &[String]) -> usize {
    actions
        .iter()
        .filter(|a| a.as_str() == REFUND_ACTION_FAILED)
        .count()
}

#[test]
fn пауза_между_повторами_действия_растёт_до_часа() {
    let seconds: Vec<i64> = (1..=REFUND_ACTION_ATTEMPTS)
        .map(|n| retry_delay(n).num_seconds())
        .collect();
    assert_eq!(
        seconds,
        vec![
            60, 120, 240, 480, 960, 1_920, 3_600, 3_600, 3_600, 3_600, 3_600, 3_600
        ]
    );
    assert_eq!(retry_delay(0).num_seconds(), 60);
    assert_eq!(retry_delay(u32::MAX).num_seconds(), 3_600);
}

#[test]
fn обрезка_срока_не_продлевает_истёкшую_подписку_и_не_уходит_в_прошлое() {
    let now = DateTime::parse_from_rfc3339("2026-11-17T10:00:00.250Z")
        .unwrap()
        .with_timezone(&Utc);
    let day = chrono::Duration::days(1);
    let long = now + day * 120 + chrono::Duration::milliseconds(417);
    assert_eq!(
        cut_target(long, 30, now),
        now + day * 90 - chrono::Duration::milliseconds(250)
    );
    assert_eq!(
        cut_target(now + day * 10, 30, now),
        now - chrono::Duration::milliseconds(250)
    );
    let expired = now - day;
    assert_eq!(cut_target(expired, 30, now), expired);
    assert_eq!(cut_target(now, 30, now), now);
}

#[tokio::test]
async fn сбой_remnawave_при_подтверждении_возврата_повторяется_до_отключения_подписки() {
    let kassa = Arc::new(Kassa::default());
    let Some((kit, clock)) = clocked_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7600).await;
    let tariff = standard(&kit.services).await;
    let bought = buy(&kit, client_id, &tariff, 90, None).await;
    request_refund(&kit, admin, &bought, RefundSubscriptionAction::Disable).await;
    assert_eq!(action(&kit, &bought).await.status, RefundActionStatus::None);
    kit.remna.fail_on("disable_user", Fault::Unavailable);
    let confirmed_at = kit.services.now();
    confirm(&kit, &bought, "kr-1").await;
    assert_eq!(
        payment(&kit, bought.payment.id).await.status,
        PaymentStatus::Refunded
    );
    assert_eq!(
        remna_user(&kit, &bought.subscription).await.status,
        UserStatus::Active
    );
    let waiting = action(&kit, &bought).await;
    assert_eq!(waiting.status, RefundActionStatus::Pending);
    assert_eq!(waiting.attempts, 1);
    assert_eq!(
        waiting.next_at,
        Some(confirmed_at + chrono::Duration::seconds(60))
    );
    assert!(waiting.error.is_some());
    assert!(
        subscription_audit(&kit, &bought.subscription)
            .await
            .is_empty()
    );
    let inbox = kit.services.inbox();
    assert_eq!(
        inbox.poll_refunds(10).await.unwrap(),
        RefundPollReport::default()
    );
    assert_eq!(
        remna_user(&kit, &bought.subscription).await.status,
        UserStatus::Active
    );
    advance(&kit, &clock, chrono::Duration::seconds(61));
    assert_eq!(
        inbox.poll_refunds(10).await.unwrap(),
        RefundPollReport {
            actions_done: 1,
            ..RefundPollReport::default()
        }
    );
    assert_eq!(
        remna_user(&kit, &bought.subscription).await.status,
        UserStatus::Disabled
    );
    let done = action(&kit, &bought).await;
    assert_eq!(done.status, RefundActionStatus::Done);
    assert_eq!(done.next_at, None);
    assert_eq!(done.error, None);
    assert_eq!(
        subscription_audit(&kit, &bought.subscription).await,
        vec!["subscription.refund_action".to_string()]
    );
    let calls = kit.remna.calls_of("disable_user").len();
    assert_eq!(calls, 2);
    advance(&kit, &clock, chrono::Duration::hours(3));
    assert_eq!(
        inbox.poll_refunds(10).await.unwrap(),
        RefundPollReport::default()
    );
    assert_eq!(kit.remna.calls_of("disable_user").len(), calls);
    assert!(inbox.refund_actions_review(50).await.unwrap().is_empty());
    assert_eq!(bought.order.subscription_id, Some(bought.subscription.id));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn постоянный_сбой_действия_с_подпиской_уходит_на_ручное_действие_с_аудитом() {
    let kassa = Arc::new(Kassa::default());
    let Some((kit, clock)) = clocked_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7601).await;
    let tariff = standard(&kit.services).await;
    let bought = buy(&kit, client_id, &tariff, 90, None).await;
    let request_id = request_refund(&kit, admin, &bought, RefundSubscriptionAction::Disable).await;
    for _ in 0..REFUND_ACTION_ATTEMPTS {
        kit.remna.fail_on("disable_user", Fault::Unavailable);
    }
    confirm(&kit, &bought, "kr-1").await;
    let inbox = kit.services.inbox();
    let mut gave_up = 0;
    for _ in 0..(REFUND_ACTION_ATTEMPTS * 2) {
        advance(&kit, &clock, chrono::Duration::hours(2));
        gave_up += inbox.poll_refunds(10).await.unwrap().actions_failed;
    }
    assert_eq!(gave_up, 1);
    assert_eq!(
        kit.remna.calls_of("disable_user").len(),
        REFUND_ACTION_ATTEMPTS as usize
    );
    assert_eq!(
        remna_user(&kit, &bought.subscription).await.status,
        UserStatus::Active
    );
    let failed = action(&kit, &bought).await;
    assert_eq!(failed.status, RefundActionStatus::Failed);
    assert_eq!(failed.attempts, REFUND_ACTION_ATTEMPTS);
    assert_eq!(failed.next_at, None);
    let audit = subscription_audit(&kit, &bought.subscription).await;
    assert_eq!(audit, vec![REFUND_ACTION_FAILED.to_string()]);
    let review = inbox.refund_actions_review(50).await.unwrap();
    assert_eq!(review.len(), 1, "{review:?}");
    assert_eq!(review[0].request_id, request_id);
    assert_eq!(review[0].payment_id, bought.payment.id);
    assert_eq!(review[0].order_id, bought.order.id);
    assert_eq!(review[0].client_id, client_id);
    assert_eq!(review[0].subscription_id, Some(bought.subscription.id));
    assert_eq!(review[0].action, RefundSubscriptionAction::Disable);
    assert_eq!(review[0].attempts, REFUND_ACTION_ATTEMPTS);
    assert_eq!(review[0].requested_by, Some(admin));
    assert!(review[0].error.is_some());
    assert_eq!(
        inbox
            .close_refund_action(request_id, admin, "  ")
            .await
            .unwrap_err()
            .code(),
        "refund.action_note"
    );
    assert_eq!(
        inbox.retry_refund_action(request_id, admin).await.unwrap(),
        RefundActionOutcome::Done
    );
    assert_eq!(
        remna_user(&kit, &bought.subscription).await.status,
        UserStatus::Disabled
    );
    assert_eq!(action(&kit, &bought).await.status, RefundActionStatus::Done);
    let audit = subscription_audit(&kit, &bought.subscription).await;
    assert_eq!(failed_audits(&audit), 1);
    assert_eq!(
        audit[1..],
        [
            "subscription.refund_action_retry".to_string(),
            "subscription.refund_action".to_string()
        ]
    );
    assert!(inbox.refund_actions_review(50).await.unwrap().is_empty());
    assert_eq!(
        inbox
            .retry_refund_action(request_id, admin)
            .await
            .unwrap_err()
            .code(),
        "refund.action_not_failed"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn обрезка_срока_при_возврате_не_применяется_дважды() {
    let kassa = Arc::new(Kassa::default());
    let Some((kit, clock)) = clocked_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7602).await;
    let tariff = standard(&kit.services).await;
    let first = buy(&kit, client_id, &tariff, 90, None).await;
    let renewal = buy(&kit, client_id, &tariff, 30, Some(&first.subscription)).await;
    assert_eq!(renewal.subscription.id, first.subscription.id);
    let before = remna_user(&kit, &renewal.subscription).await.expire_at;
    let expected = before - chrono::Duration::days(30);
    request_refund(&kit, admin, &renewal, RefundSubscriptionAction::Cut).await;
    kit.remna.fail_on("update_user", Fault::TimeoutAfterApply);
    confirm(&kit, &renewal, "kr-1").await;
    let waiting = action(&kit, &renewal).await;
    assert_eq!(waiting.status, RefundActionStatus::Pending);
    assert_eq!(waiting.attempts, 1);
    assert!(waiting.base_expire_at.is_some());
    assert_eq!(
        waiting.target_expire_at.map(|t| t.timestamp()),
        Some(expected.timestamp())
    );
    let applied = remna_user(&kit, &renewal.subscription).await.expire_at;
    assert_eq!(applied.timestamp(), expected.timestamp());
    let updates = kit.remna.calls_of("update_user").len();
    advance(&kit, &clock, chrono::Duration::seconds(61));
    assert_eq!(
        kit.services
            .inbox()
            .poll_refunds(10)
            .await
            .unwrap()
            .actions_done,
        1
    );
    assert_eq!(kit.remna.calls_of("update_user").len(), updates);
    assert_eq!(
        remna_user(&kit, &renewal.subscription).await.expire_at,
        applied
    );
    assert_eq!(
        action(&kit, &renewal).await.status,
        RefundActionStatus::Done
    );
    assert_eq!(
        subscription_audit(&kit, &renewal.subscription).await,
        vec!["subscription.refund_action".to_string()]
    );
    let mirror = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.subscriptions()
            .get(renewal.subscription.id)
            .await
            .unwrap()
            .mirror
    };
    assert_eq!(
        mirror.expire_at.map(|t| t.timestamp()),
        Some(expected.timestamp())
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn обрезка_после_изменения_срока_ждёт_ручного_решения() {
    let kassa = Arc::new(Kassa::default());
    let Some((kit, clock)) = clocked_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7603).await;
    let tariff = standard(&kit.services).await;
    let first = buy(&kit, client_id, &tariff, 90, None).await;
    let renewal = buy(&kit, client_id, &tariff, 30, Some(&first.subscription)).await;
    let before = remna_user(&kit, &renewal.subscription).await.expire_at;
    let request_id = request_refund(&kit, admin, &renewal, RefundSubscriptionAction::Cut).await;
    kit.remna.fail_on("update_user", Fault::Unavailable);
    confirm(&kit, &renewal, "kr-1").await;
    assert_eq!(
        remna_user(&kit, &renewal.subscription).await.expire_at,
        before
    );
    let moved = before + chrono::Duration::days(90);
    set_user(&kit, &renewal.subscription, |user| user.expire_at = moved).await;
    advance(&kit, &clock, chrono::Duration::seconds(61));
    let inbox = kit.services.inbox();
    assert_eq!(inbox.poll_refunds(10).await.unwrap().actions_failed, 1);
    assert_eq!(
        remna_user(&kit, &renewal.subscription).await.expire_at,
        moved
    );
    let failed = action(&kit, &renewal).await;
    assert_eq!(failed.status, RefundActionStatus::Failed);
    assert_eq!(failed.attempts, 2);
    assert_eq!(
        subscription_audit(&kit, &renewal.subscription).await,
        vec![REFUND_ACTION_FAILED.to_string()]
    );
    assert_eq!(
        inbox.retry_refund_action(request_id, admin).await.unwrap(),
        RefundActionOutcome::Failed
    );
    assert_eq!(
        remna_user(&kit, &renewal.subscription).await.expire_at,
        moved
    );
    inbox
        .close_refund_action(request_id, admin, "Срок укоротили вручную в панели")
        .await
        .unwrap();
    assert_eq!(
        action(&kit, &renewal).await.status,
        RefundActionStatus::Done
    );
    assert!(inbox.refund_actions_review(50).await.unwrap().is_empty());
    let audit = subscription_audit(&kit, &renewal.subscription).await;
    assert_eq!(
        audit.last().map(String::as_str),
        Some("subscription.refund_action_closed")
    );
    assert_eq!(failed_audits(&audit), 2);
    assert_eq!(
        inbox
            .close_refund_action(request_id, admin, "ещё раз")
            .await
            .unwrap_err()
            .code(),
        "refund.action_closed"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn возврат_с_немедленным_ответом_кассы_обрезает_срок_сразу() {
    let kassa = Arc::new(Kassa::default());
    let Some((kit, _clock)) = clocked_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7604).await;
    let tariff = standard(&kit.services).await;
    let first = buy(&kit, client_id, &tariff, 90, None).await;
    let renewal = buy(&kit, client_id, &tariff, 30, Some(&first.subscription)).await;
    let before = remna_user(&kit, &renewal.subscription).await.expire_at;
    *kassa.refund_completed.lock().unwrap() = true;
    let refunded = kit
        .services
        .inbox()
        .refund(
            renewal.payment.id,
            &refund_command(None, RefundSubscriptionAction::Cut, None),
            admin,
        )
        .await
        .unwrap();
    assert!(!refunded.pending);
    assert_eq!(refunded.subscription_error, None);
    assert_eq!(
        remna_user(&kit, &renewal.subscription)
            .await
            .expire_at
            .timestamp(),
        (before - chrono::Duration::days(30)).timestamp()
    );
    assert_eq!(
        subscription_audit(&kit, &renewal.subscription).await,
        vec!["subscription.refund_action".to_string()]
    );
    assert!(provider_refunds(&kit, renewal.payment.id).await.is_empty());
    kit.db.cleanup().await;
}

async fn paid_undelivered_renewal(
    kit: &Kit,
    client_id: ClientId,
    tariff: &Tariff,
    current: &Subscription,
) -> (Payment, Order) {
    let result = kit
        .services
        .checkout()
        .start(
            client_id,
            &CheckoutRequest {
                cart: cart(tariff, 30, Some(current)),
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
    let invoice = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments().by_public_id(payment_id).await.unwrap()
    };
    kit.services
        .inbox()
        .apply_outcome(KASSA, &outcome(&invoice, ProviderStatus::Paid))
        .await
        .unwrap();
    let renewal = order(&kit.services, invoice.order_id).await;
    assert_eq!(renewal.status, OrderStatus::Paid);
    (payment(kit, invoice.id).await, renewal)
}

#[tokio::test]
async fn обрезка_срока_после_ожидающего_возврата_не_трогает_невыданный_заказ() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 9602).await;
    let tariff = standard(&kit.services).await;
    let first = buy(&kit, client_id, &tariff, 90, None).await;
    let before = remna_user(&kit, &first.subscription).await.expire_at;
    let (paid, renewal) =
        paid_undelivered_renewal(&kit, client_id, &tariff, &first.subscription).await;
    let requested = kit
        .services
        .inbox()
        .refund(
            paid.id,
            &refund_command(None, RefundSubscriptionAction::Cut, Some("не продлилось")),
            admin,
        )
        .await
        .unwrap();
    assert!(requested.pending);
    let held = kit.services.fulfillment().run(renewal.id).await.unwrap();
    assert!(matches!(held, FulfillmentOutcome::Retry { .. }), "{held:?}");
    let confirmed = WebhookOutcome {
        event_key: "refund:kr-1".into(),
        ..outcome(&paid, ProviderStatus::Refunded)
    };
    kit.services
        .inbox()
        .apply_outcome(KASSA, &confirmed)
        .await
        .unwrap();
    let refunded = order(&kit.services, renewal.id).await;
    assert_eq!(refunded.status, OrderStatus::Refunded);
    assert!(refunded.fulfilled_at.is_none());
    assert_eq!(
        remna_user(&kit, &first.subscription).await.expire_at,
        before,
        "заказ не выдавался: срок, оплаченный раньше, не укорачивается"
    );
    let actions = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments()
            .provider_refunds(paid.id)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.action.status)
            .collect::<Vec<_>>()
    };
    assert_eq!(actions, vec![RefundActionStatus::Done]);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn обрезка_срока_при_немедленном_возврате_не_трогает_невыданный_заказ() {
    let kassa = Arc::new(Kassa::default());
    *kassa.refund_completed.lock().unwrap() = true;
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 9603).await;
    let tariff = standard(&kit.services).await;
    let first = buy(&kit, client_id, &tariff, 90, None).await;
    let before = remna_user(&kit, &first.subscription).await.expire_at;
    let (paid, renewal) =
        paid_undelivered_renewal(&kit, client_id, &tariff, &first.subscription).await;
    let done = kit
        .services
        .inbox()
        .refund(
            paid.id,
            &refund_command(None, RefundSubscriptionAction::Cut, Some("не продлилось")),
            admin,
        )
        .await
        .unwrap();
    assert!(!done.pending);
    assert_eq!(
        order(&kit.services, renewal.id).await.status,
        OrderStatus::Refunded
    );
    assert_eq!(
        remna_user(&kit, &first.subscription).await.expire_at,
        before,
        "заказ не выдавался: срок, оплаченный раньше, не укорачивается"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заказ_с_планом_и_ожидающим_возвратом_не_держит_следующую_покупку() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 9604).await;
    let tariff = standard(&kit.services).await;
    let first = buy(&kit, client_id, &tariff, 90, None).await;
    let before = remna_user(&kit, &first.subscription).await.expire_at;
    let (paid, stuck) =
        paid_undelivered_renewal(&kit, client_id, &tariff, &first.subscription).await;
    kit.remna.fail_on("update_user", Fault::Timeout);
    let attempt = kit.services.fulfillment().run(stuck.id).await.unwrap();
    assert!(
        matches!(attempt, FulfillmentOutcome::Retry { .. }),
        "{attempt:?}"
    );
    assert!(order(&kit.services, stuck.id).await.plan.is_some());
    let requested = kit
        .services
        .inbox()
        .refund(
            paid.id,
            &refund_command(None, RefundSubscriptionAction::Keep, Some("не продлилось")),
            admin,
        )
        .await
        .unwrap();
    assert!(requested.pending);
    let (_, again) = paid_undelivered_renewal(&kit, client_id, &tariff, &first.subscription).await;
    let delivered = kit.services.fulfillment().run(again.id).await.unwrap();
    assert!(
        matches!(delivered, FulfillmentOutcome::Done { .. }),
        "повторная покупка не ждёт решения кассы по возврату: {delivered:?}"
    );
    assert_eq!(
        remna_user(&kit, &first.subscription).await.expire_at,
        before + chrono::Duration::days(30)
    );
    let holds = {
        let mut conn = kit.services.store().conn().await.unwrap();
        crate::fulfillment::holds_autorenew(&mut conn, stuck.id)
            .await
            .unwrap()
    };
    assert!(
        !holds,
        "заказ с ожидающим возвратом не держит автопродление"
    );
    let confirmed = WebhookOutcome {
        event_key: "refund:kr-1".into(),
        ..outcome(&paid, ProviderStatus::Refunded)
    };
    kit.services
        .inbox()
        .apply_outcome(KASSA, &confirmed)
        .await
        .unwrap();
    assert_eq!(
        order(&kit.services, stuck.id).await.status,
        OrderStatus::Refunded
    );
    assert_eq!(
        remna_user(&kit, &first.subscription).await.expire_at,
        before + chrono::Duration::days(30)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn обрезка_срока_снимает_дни_заказа_который_применился_без_ответа_панели() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 9605).await;
    let tariff = standard(&kit.services).await;
    let first = buy(&kit, client_id, &tariff, 90, None).await;
    let before = remna_user(&kit, &first.subscription).await.expire_at;
    let (paid, renewal) =
        paid_undelivered_renewal(&kit, client_id, &tariff, &first.subscription).await;
    kit.remna.fail_on("update_user", Fault::TimeoutAfterApply);
    let attempt = kit.services.fulfillment().run(renewal.id).await.unwrap();
    assert!(
        matches!(attempt, FulfillmentOutcome::Retry { .. }),
        "{attempt:?}"
    );
    let extended = remna_user(&kit, &first.subscription).await.expire_at;
    assert!(
        extended > before,
        "продление должно было примениться в панели"
    );
    let requested = kit
        .services
        .inbox()
        .refund(
            paid.id,
            &refund_command(None, RefundSubscriptionAction::Cut, Some("не продлилось")),
            admin,
        )
        .await
        .unwrap();
    assert!(requested.pending);
    let confirmed = WebhookOutcome {
        event_key: "refund:kr-1".into(),
        ..outcome(&paid, ProviderStatus::Refunded)
    };
    kit.services
        .inbox()
        .apply_outcome(KASSA, &confirmed)
        .await
        .unwrap();
    assert_eq!(
        remna_user(&kit, &first.subscription)
            .await
            .expire_at
            .timestamp(),
        before.timestamp(),
        "дни заказа, которые успели попасть в панель, снимаются"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отключение_после_возврата_ждёт_админа_если_за_время_сбоя_оплачен_новый_срок() {
    let kassa = Arc::new(Kassa::default());
    let Some((kit, clock)) = clocked_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7610).await;
    let tariff = standard(&kit.services).await;
    let bought = buy(&kit, client_id, &tariff, 90, None).await;
    let alerts = kit.services.alerts();
    alerts.collect().await.unwrap();
    let request_id = request_refund(&kit, admin, &bought, RefundSubscriptionAction::Disable).await;
    kit.remna.fail_on("disable_user", Fault::Unavailable);
    confirm(&kit, &bought, "kr-1").await;
    assert_eq!(
        action(&kit, &bought).await.status,
        RefundActionStatus::Pending
    );
    let renewal = buy(&kit, client_id, &tariff, 30, Some(&bought.subscription)).await;
    assert_eq!(renewal.subscription.id, bought.subscription.id);
    let extended = remna_user(&kit, &bought.subscription).await.expire_at;
    advance(&kit, &clock, chrono::Duration::seconds(61));
    let inbox = kit.services.inbox();
    assert_eq!(
        inbox.poll_refunds(10).await.unwrap(),
        RefundPollReport {
            actions_failed: 1,
            ..RefundPollReport::default()
        }
    );
    let user = remna_user(&kit, &bought.subscription).await;
    assert_eq!(user.status, UserStatus::Active);
    assert_eq!(user.expire_at, extended);
    assert_eq!(kit.remna.calls_of("disable_user").len(), 1);
    let failed = action(&kit, &bought).await;
    assert_eq!(failed.status, RefundActionStatus::Failed);
    let error = failed.error.unwrap();
    assert!(
        error.contains(&format!("заказ №{}", renewal.order.id)),
        "{error}"
    );
    assert!(error.contains("подписку не отключили"), "{error}");
    let review = inbox.refund_actions_review(50).await.unwrap();
    assert_eq!(review.len(), 1, "{review:?}");
    assert_eq!(review[0].request_id, request_id);
    assert_eq!(review[0].error.as_deref(), Some(error.as_str()));
    assert_eq!(
        subscription_audit(&kit, &bought.subscription).await,
        vec![REFUND_ACTION_FAILED.to_string()]
    );
    let got = alerts.collect().await.unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].kind, AlertKind::Refunds);
    assert!(
        got[0]
            .html
            .contains(&format!("Возврат №{request_id} не завершён")),
        "{}",
        got[0].html
    );
    assert!(
        got[0]
            .html
            .contains("Причина: После запроса возврата по подписке прошла новая оплата"),
        "{}",
        got[0].html
    );
    advance(&kit, &clock, chrono::Duration::hours(3));
    assert_eq!(
        inbox.poll_refunds(10).await.unwrap(),
        RefundPollReport::default()
    );
    inbox
        .close_refund_action(
            request_id,
            admin,
            "Клиент продлил подписку, оставили как есть",
        )
        .await
        .unwrap();
    assert_eq!(action(&kit, &bought).await.status, RefundActionStatus::Done);
    assert_eq!(
        remna_user(&kit, &bought.subscription).await.status,
        UserStatus::Active
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn укорочение_после_возврата_ждёт_админа_пока_новый_срок_оплачен() {
    let kassa = Arc::new(Kassa::default());
    let Some((kit, _clock)) = clocked_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7611).await;
    let tariff = standard(&kit.services).await;
    let bought = buy(&kit, client_id, &tariff, 90, None).await;
    let request_id = request_refund(&kit, admin, &bought, RefundSubscriptionAction::Cut).await;
    let renewal = buy(&kit, client_id, &tariff, 30, Some(&bought.subscription)).await;
    let extended = remna_user(&kit, &bought.subscription).await.expire_at;
    let updates = kit.remna.calls_of("update_user").len();
    confirm(&kit, &bought, "kr-1").await;
    let failed = action(&kit, &bought).await;
    assert_eq!(failed.status, RefundActionStatus::Failed);
    assert_eq!(failed.attempts, 1);
    assert_eq!(failed.base_expire_at, None);
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|e| e.contains("срок подписки не укоротили")
                && e.contains(&format!("заказ №{}", renewal.order.id))),
        "{failed:?}"
    );
    assert_eq!(kit.remna.calls_of("update_user").len(), updates);
    assert_eq!(
        remna_user(&kit, &bought.subscription).await.expire_at,
        extended
    );
    let inbox = kit.services.inbox();
    assert_eq!(
        inbox.retry_refund_action(request_id, admin).await.unwrap(),
        RefundActionOutcome::Failed
    );
    assert_eq!(
        remna_user(&kit, &bought.subscription).await.expire_at,
        extended
    );
    *kassa.refund_completed.lock().unwrap() = true;
    let returned = inbox
        .refund(
            renewal.payment.id,
            &refund_command(
                None,
                RefundSubscriptionAction::Keep,
                Some("Вернули и продление"),
            ),
            admin,
        )
        .await
        .unwrap();
    assert!(!returned.pending);
    assert_eq!(
        inbox.retry_refund_action(request_id, admin).await.unwrap(),
        RefundActionOutcome::Done
    );
    assert_eq!(
        remna_user(&kit, &bought.subscription)
            .await
            .expire_at
            .timestamp(),
        (extended - chrono::Duration::days(90)).timestamp()
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn покупка_до_запроса_возврата_не_мешает_отключению() {
    let kassa = Arc::new(Kassa::default());
    let Some((kit, _clock)) = clocked_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7612).await;
    let tariff = standard(&kit.services).await;
    let first = buy(&kit, client_id, &tariff, 90, None).await;
    let renewal = buy(&kit, client_id, &tariff, 30, Some(&first.subscription)).await;
    assert_eq!(renewal.subscription.id, first.subscription.id);
    request_refund(&kit, admin, &first, RefundSubscriptionAction::Disable).await;
    confirm(&kit, &first, "kr-1").await;
    assert_eq!(action(&kit, &first).await.status, RefundActionStatus::Done);
    assert_eq!(
        remna_user(&kit, &first.subscription).await.status,
        UserStatus::Disabled
    );
    kit.db.cleanup().await;
}
