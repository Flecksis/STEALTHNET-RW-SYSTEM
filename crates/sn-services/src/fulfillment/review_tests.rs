use super::*;
use crate::Actor;
use crate::checkout::testkit::*;
use chrono::Duration as Days;
use sn_domain::ids::AdminId;
use sn_domain::subscription::SubscriptionStatus;
use sn_domain::tariff::Tariff;
use sn_domain::traffic::{GIB, TrafficPolicy, TrafficReset};
use sn_remnawave::fake::Fault;
use sn_store::PlanStage;

const ADMIN: Actor = Actor::Admin(AdminId(1));

async fn plan_stage(kit: &Kit, order_id: OrderId) -> PlanStage {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.orders().plan_state(order_id).await.unwrap().stage()
}

async fn review_reason(kit: &Kit, order_id: OrderId) -> Option<String> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.orders().review_reason(order_id).await.unwrap()
}

async fn order_audit(kit: &Kit, order_id: OrderId, action: &str) -> Vec<sn_store::AuditRecord> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.audit()
        .for_entity("order", order_id.0, 20)
        .await
        .unwrap()
        .into_iter()
        .filter(|record| record.action == action)
        .collect()
}

async fn subscription_now(kit: &Kit, id: SubscriptionId) -> Subscription {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.subscriptions().get(id).await.unwrap()
}

async fn hold_for_review(kit: &Kit, order_id: OrderId) {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.orders()
        .mark_review(order_id, "выдача не завершилась")
        .await
        .unwrap();
}

fn fulfilled_notice(queued: &[JobPayload], order: &Order) -> Option<serde_json::Value> {
    queued.iter().find_map(|job| match job {
        JobPayload::Notify {
            template, params, ..
        } if template == FULFILLED_TEMPLATE
            && params["order_id"] == serde_json::json!(order.public_id) =>
        {
            Some(params.clone())
        }
        _ => None,
    })
}

fn panel_writes(kit: &Kit) -> usize {
    kit.remna
        .calls()
        .iter()
        .filter(|call| !call.starts_with("get_"))
        .count()
}

struct Renewal {
    client_id: ClientId,
    tariff: Tariff,
    subscription: Subscription,
    expire: DateTime<Utc>,
    order: Order,
}

async fn unsent_renewal(kit: &Kit, telegram_id: i64) -> Renewal {
    let client_id = client(&kit.services, telegram_id).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(kit, client_id, &tariff, 30).await;
    let expire = moment(Days::days(10));
    let subscription = expire_in(kit, &subscription, expire, SubscriptionStatus::Active).await;
    let order = buy_with_balance(
        &kit.services,
        client_id,
        cart(&tariff, 30, Some(&subscription)),
    )
    .await;
    kit.remna.fail_on("update_user", Fault::Timeout);
    let first = kit.services.fulfillment().run(order.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    assert_eq!(plan_stage(kit, order.id).await, PlanStage::Saved);
    Renewal {
        client_id,
        tariff,
        subscription,
        expire,
        order: self::order(&kit.services, order.id).await,
    }
}

async fn unblock_tariff(kit: &Kit) -> Tariff {
    let category = category(&kit.services, "Unblock").await;
    tariff(
        &kit.services,
        category,
        "Unblock",
        TrafficPolicy {
            limit_bytes: Some(300 * GIB),
            reset: TrafficReset::Month,
        },
        vec![squad(3)],
    )
    .await
}

#[tokio::test]
async fn повторить_выдачу_сама_пересчитывает_устаревший_план_если_срок_не_менялся() {
    let Some(kit) = kit().await else { return };
    let renewal = unsent_renewal(&kit, 311).await;
    hold_for_review(&kit, renewal.order.id).await;
    assert_eq!(plan_stage(&kit, renewal.order.id).await, PlanStage::Stale);
    let fulfillment = kit.services.fulfillment();
    let queued = fulfillment
        .redeliver(renewal.order.id, ADMIN)
        .await
        .unwrap();
    assert!(queued.recalculated);
    assert_eq!(plan_stage(&kit, renewal.order.id).await, PlanStage::None);
    assert!(
        jobs(&kit.services)
            .await
            .contains(&JobPayload::FulfillOrder {
                order_id: renewal.order.id
            })
    );
    let audit = order_audit(&kit, renewal.order.id, "order.redeliver").await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].actor_kind, "admin");
    assert_eq!(audit[0].details["recalculated"], true);
    let base: DateTime<Utc> =
        serde_json::from_value(audit[0].details["previous_plan"]["base_expire_at"].clone())
            .unwrap();
    assert_eq!(base, renewal.expire);
    let done = fulfillment.run(renewal.order.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(
        remna_user(&kit, &renewal.subscription).await.expire_at,
        renewal.expire + Days::days(30)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повторить_выдачу_не_пересчитывает_если_срок_изменился_или_панель_молчит() {
    let Some(kit) = kit().await else { return };
    let renewal = unsent_renewal(&kit, 312).await;
    kit.services
        .subscriptions()
        .add_days(renewal.subscription.id, 7, ADMIN)
        .await
        .unwrap();
    hold_for_review(&kit, renewal.order.id).await;
    let fulfillment = kit.services.fulfillment();
    let changed = fulfillment
        .redeliver(renewal.order.id, ADMIN)
        .await
        .unwrap_err();
    assert_eq!(changed.code(), "order.plan_stale");
    assert!(changed.to_string().contains("изменился"), "{changed}");
    kit.remna.fail_on("get_user", Fault::Unavailable);
    let silent = fulfillment
        .redeliver(renewal.order.id, ADMIN)
        .await
        .unwrap_err();
    assert_eq!(silent.code(), "order.panel_unavailable");
    assert_eq!(plan_stage(&kit, renewal.order.id).await, PlanStage::Stale);
    assert!(
        order_audit(&kit, renewal.order.id, "order.redeliver")
            .await
            .is_empty()
    );
    assert_eq!(
        remna_user(&kit, &renewal.subscription).await.expire_at,
        renewal.expire + Days::days(7)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повторить_выдачу_пересчитывает_план_создания_пока_пользователя_нет() {
    let Some(kit) = kit().await else { return };
    let tariff = standard(&kit.services).await;
    let fulfillment = kit.services.fulfillment();
    let mut orders = Vec::new();
    for (telegram_id, fault) in [(313, Fault::Timeout), (314, Fault::TimeoutAfterApply)] {
        let client_id = client(&kit.services, telegram_id).await;
        credit(&kit.services, client_id, 1_000).await;
        let bought = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
        kit.remna.fail_on("create_user", fault);
        let first = fulfillment.run(bought.id).await.unwrap();
        assert!(
            matches!(first, FulfillmentOutcome::Retry { .. }),
            "{first:?}"
        );
        hold_for_review(&kit, bought.id).await;
        assert_eq!(plan_stage(&kit, bought.id).await, PlanStage::Stale);
        orders.push(bought);
    }
    assert_eq!(kit.remna.users().len(), 1);
    let queued = fulfillment.redeliver(orders[0].id, ADMIN).await.unwrap();
    assert!(queued.recalculated);
    let done = fulfillment.run(orders[0].id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    assert_eq!(kit.remna.users().len(), 2);
    let created = fulfillment
        .redeliver(orders[1].id, ADMIN)
        .await
        .unwrap_err();
    assert_eq!(created.code(), "order.plan_stale");
    assert_eq!(plan_stage(&kit, orders[1].id).await, PlanStage::Stale);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn после_разблокировки_оплаченный_заказ_выдаётся_сам() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 315).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let bought = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    kit.services
        .clients()
        .block(client_id, Some("проверка"), ADMIN)
        .await
        .unwrap();
    let fulfillment = kit.services.fulfillment();
    let held = fulfillment.run(bought.id).await.unwrap();
    assert!(
        matches!(&held, FulfillmentOutcome::Review { reason }
            if reason.starts_with(BLOCKED_HOLD) && reason.contains("выдастся сам")),
        "{held:?}"
    );
    jobs(&kit.services).await;
    kit.services
        .clients()
        .unblock(client_id, ADMIN)
        .await
        .unwrap();
    let audit = order_audit(&kit, bought.id, "order.redeliver").await;
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0].actor_kind, "system");
    assert_eq!(audit[0].details["reason"], UNBLOCKED_CAUSE);
    assert_eq!(audit[0].details["recalculated"], false);
    let done = fulfillment.run(bought.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    let delivered = self::order(&kit.services, bought.id).await;
    assert_eq!(delivered.status, OrderStatus::Fulfilled);
    assert!(fulfilled_notice(&jobs(&kit.services).await, &delivered).is_some());
    let subscription = subscription(&kit.services, &delivered).await;
    assert_eq!(
        remna_user(&kit, &subscription).await.status,
        sn_remnawave::UserStatus::Active
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn после_разблокировки_заказ_с_возвратом_или_изменённым_сроком_ждёт_админа() {
    let Some(kit) = kit().await else { return };
    let renewal = unsent_renewal(&kit, 316).await;
    let refunded = buy_with_balance(
        &kit.services,
        renewal.client_id,
        cart(&renewal.tariff, 90, Some(&renewal.subscription)),
    )
    .await;
    kit.services
        .clients()
        .block(renewal.client_id, Some("проверка"), ADMIN)
        .await
        .unwrap();
    let fulfillment = kit.services.fulfillment();
    for order_id in [renewal.order.id, refunded.id] {
        let held = fulfillment.run(order_id).await.unwrap();
        assert!(
            matches!(&held, FulfillmentOutcome::Review { reason } if reason.starts_with(BLOCKED_HOLD)),
            "{held:?}"
        );
    }
    assert_eq!(plan_stage(&kit, renewal.order.id).await, PlanStage::Stale);
    sqlx::query(
        "UPDATE payments SET status = 'partially_refunded', refunded_minor = 100 WHERE order_id = $1",
    )
    .bind(refunded.id.0)
    .execute(kit.services.store().pool())
    .await
    .unwrap();
    set_user(&kit, &renewal.subscription, |user| {
        user.expire_at = renewal.expire + Days::days(3);
    })
    .await;
    let blocked_reason = review_reason(&kit, refunded.id).await.unwrap();
    kit.services
        .clients()
        .unblock(renewal.client_id, ADMIN)
        .await
        .unwrap();
    for order_id in [renewal.order.id, refunded.id] {
        assert!(
            order_audit(&kit, order_id, "order.redeliver")
                .await
                .is_empty()
        );
        assert_eq!(
            self::order(&kit.services, order_id).await.status,
            OrderStatus::Review
        );
    }
    let kept = review_reason(&kit, renewal.order.id).await.unwrap();
    assert!(
        kept.starts_with(UNBLOCKED_CAUSE) && kept.contains("Пересчитать и выдать"),
        "{kept}"
    );
    assert_eq!(plan_stage(&kit, renewal.order.id).await, PlanStage::Stale);
    assert_eq!(
        review_reason(&kit, refunded.id).await.unwrap(),
        blocked_reason
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn разблокировка_до_автоматического_возврата_не_останавливает_выдачу() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 317).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let bought = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    for blocked in [true, false] {
        if blocked {
            kit.services
                .clients()
                .block(client_id, Some("проверка"), ADMIN)
                .await
                .unwrap();
        } else {
            kit.services
                .clients()
                .unblock(client_id, ADMIN)
                .await
                .unwrap();
        }
    }
    let fulfillment = kit.services.fulfillment();
    let outcome = fulfillment
        .blocked_order(&bought, format!("{BLOCKED_HOLD} {}", bought.public_id))
        .await
        .unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Retry { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        self::order(&kit.services, bought.id).await.status,
        OrderStatus::Paid
    );
    let done = fulfillment.run(bought.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заказ_без_оплаты_после_разблокировки_не_застревает_на_проверке() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 319).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let free = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    sqlx::query("UPDATE payments SET amount_minor = 0 WHERE order_id = $1")
        .bind(free.id.0)
        .execute(kit.services.store().pool())
        .await
        .unwrap();
    let fulfillment = kit.services.fulfillment();
    let outcome = fulfillment
        .blocked_order(&free, format!("{BLOCKED_HOLD} {}", free.public_id))
        .await
        .unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Retry { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        self::order(&kit.services, free.id).await.status,
        OrderStatus::Paid
    );
    kit.services
        .clients()
        .block(client_id, Some("проверка"), ADMIN)
        .await
        .unwrap();
    let held = fulfillment.run(free.id).await.unwrap();
    assert!(
        matches!(&held, FulfillmentOutcome::Review { reason }
            if reason.starts_with(BLOCKED_HOLD) && reason.contains("выдастся сам")),
        "{held:?}"
    );
    assert!(
        review_reason(&kit, free.id)
            .await
            .is_some_and(|reason| reason.starts_with(BLOCKED_HOLD))
    );
    jobs(&kit.services).await;
    kit.services
        .clients()
        .unblock(client_id, ADMIN)
        .await
        .unwrap();
    assert_eq!(order_audit(&kit, free.id, "order.redeliver").await.len(), 1);
    let done = fulfillment.run(free.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отметить_выданным_подтягивает_срок_и_тариф_из_панели_и_пишет_клиенту() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 318).await;
    let standard = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &standard, 30).await;
    let unblock = unblock_tariff(&kit).await;
    let change = buy_with_balance(
        &kit.services,
        client_id,
        cart(&unblock, 90, Some(&subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    kit.remna.fail_on("update_user", Fault::Timeout);
    let first = fulfillment.run(change.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    hold_for_review(&kit, change.id).await;
    let applied = moment(Days::days(90));
    let user = remna_user(&kit, &subscription).await;
    let mut manual = user.clone();
    manual.expire_at = applied;
    manual.traffic_limit_bytes = 300 * GIB as i64;
    manual.updated_at = Utc::now();
    kit.remna.insert_user(manual);
    assert_eq!(
        subscription_now(&kit, subscription.id).await.tariff_id,
        Some(standard.id)
    );
    jobs(&kit.services).await;
    let writes = panel_writes(&kit);
    let closed = fulfillment
        .mark_fulfilled(change.id, "Перевели на новый тариф вручную в панели", ADMIN)
        .await
        .unwrap();
    assert_eq!(closed.status, OrderStatus::Fulfilled);
    assert_eq!(panel_writes(&kit), writes, "в панель ничего не пишется");
    let current = subscription_now(&kit, subscription.id).await;
    assert_eq!(current.mirror.expire_at, Some(applied));
    assert_eq!(current.mirror.traffic_limit_bytes, Some(300 * GIB));
    assert_eq!(current.tariff_id, Some(unblock.id));
    let period = unblock.periods.iter().find(|p| p.days == 90).unwrap().id;
    assert_eq!(current.autorenew.period_id, Some(period));
    let notice = fulfilled_notice(&jobs(&kit.services).await, &closed).unwrap();
    assert_eq!(notice["expire_at"], serde_json::json!(applied));
    assert_eq!(notice["tariff"], "Unblock");
    assert_eq!(
        notice["subscription_id"],
        serde_json::json!(subscription.id)
    );
    let audit = order_audit(&kit, change.id, "order.mark_fulfilled").await;
    assert_eq!(audit.len(), 1);
    let synced: DateTime<Utc> =
        serde_json::from_value(audit[0].details["panel"]["expire_at"].clone()).unwrap();
    assert_eq!(synced, applied);
    assert_eq!(audit[0].details["notified"], true);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отметить_выданным_не_возвращает_тариф_после_более_поздней_выдачи() {
    let Some(kit) = kit().await else { return };
    let renewal = unsent_renewal(&kit, 319).await;
    hold_for_review(&kit, renewal.order.id).await;
    let unblock = unblock_tariff(&kit).await;
    let change = buy_with_balance(
        &kit.services,
        renewal.client_id,
        cart(&unblock, 30, Some(&renewal.subscription)),
    )
    .await;
    let fulfillment = kit.services.fulfillment();
    let done = fulfillment.run(change.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
    let panel = remna_user(&kit, &renewal.subscription).await.expire_at;
    fulfillment
        .mark_fulfilled(renewal.order.id, "Срок уже учтён", ADMIN)
        .await
        .unwrap();
    let current = subscription_now(&kit, renewal.subscription.id).await;
    assert_eq!(current.tariff_id, Some(unblock.id));
    assert_eq!(current.mirror.expire_at, Some(panel));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отметить_выданным_без_пользователя_в_панели_только_закрывает_заказ() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 320).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let bought = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    let fulfillment = kit.services.fulfillment();
    kit.remna.fail_on("create_user", Fault::Timeout);
    let first = fulfillment.run(bought.id).await.unwrap();
    assert!(
        matches!(first, FulfillmentOutcome::Retry { .. }),
        "{first:?}"
    );
    hold_for_review(&kit, bought.id).await;
    jobs(&kit.services).await;
    let closed = fulfillment
        .mark_fulfilled(bought.id, "Выдали подписку другим способом", ADMIN)
        .await
        .unwrap();
    assert_eq!(closed.status, OrderStatus::Fulfilled);
    assert!(fulfilled_notice(&jobs(&kit.services).await, &closed).is_none());
    assert!(kit.remna.users().is_empty());
    let subscription = subscription(&kit.services, &closed).await;
    assert!(subscription.remna.is_none());
    let audit = order_audit(&kit, bought.id, "order.mark_fulfilled").await;
    assert_eq!(audit[0].details["panel"], "missing");
    assert_eq!(audit[0].details["notified"], false);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отметить_выданным_при_недоступной_панели_отклоняется() {
    let Some(kit) = kit().await else { return };
    let renewal = unsent_renewal(&kit, 321).await;
    hold_for_review(&kit, renewal.order.id).await;
    kit.remna.fail_on("get_user", Fault::Unavailable);
    let err = kit
        .services
        .fulfillment()
        .mark_fulfilled(renewal.order.id, "Срок уже учтён", ADMIN)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "order.panel_unavailable");
    assert_eq!(
        self::order(&kit.services, renewal.order.id).await.status,
        OrderStatus::Review
    );
    assert!(
        order_audit(&kit, renewal.order.id, "order.mark_fulfilled")
            .await
            .is_empty()
    );
    kit.db.cleanup().await;
}
