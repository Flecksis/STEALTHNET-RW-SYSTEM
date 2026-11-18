use super::*;
use crate::Actor;
use sn_payments::ProviderNotice;
use sn_store::PaymentNotice;

async fn notices(kit: &Kit) -> Vec<PaymentNotice> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payment_notices()
        .between(0, i64::MAX, 100)
        .await
        .unwrap()
}

async fn reload(kit: &Kit, payment: &Payment) -> Payment {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().get(payment.id).await.unwrap()
}

async fn audit_actions(kit: &Kit, payment: &Payment) -> Vec<String> {
    let mut conn = kit.services.store().conn().await.unwrap();
    sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE entity_kind = 'payment' AND entity_id = $1 ORDER BY id",
    )
    .bind(payment.id.0)
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

fn texts(queued: &[JobPayload], client_id: ClientId) -> Vec<(String, String)> {
    queued
        .iter()
        .filter_map(|job| match job {
            JobPayload::Notify {
                client_id: to,
                template,
                params,
            } if *to == client_id => Some((
                template.clone(),
                params["text"].as_str().unwrap_or_default().to_string(),
            )),
            _ => None,
        })
        .collect()
}

fn with(payment: &Payment, status: ProviderStatus, event: &str) -> WebhookOutcome {
    WebhookOutcome {
        event_key: event.into(),
        ..outcome(payment, status)
    }
}

fn noticed(mut outcome: WebhookOutcome, kind: NoticeKind, text: &str) -> WebhookOutcome {
    outcome.signal = Some(OutcomeSignal::Notice {
        notice: ProviderNotice::new(kind, text),
    });
    outcome
}

fn usd(minor: i64) -> Amount {
    Amount::new(minor, PayCurrency::Usd)
}

#[tokio::test]
async fn итоговая_недоплата_зачисляется_на_баланс_один_раз_и_клиент_узнаёт_сумму() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 8101).await;
    let tariff = standard(&kit.services).await;
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let inbox = kit.services.inbox();
    let under = with(
        &payment,
        ProviderStatus::Underpaid { received: usd(649) },
        "u-1:wrong_amount",
    );
    let result = inbox.apply_outcome(KASSA, &under).await.unwrap();
    assert_eq!(
        result,
        AcceptResult::StatusChanged {
            payment_id: payment.id,
            status: PaymentStatus::Failed,
        }
    );
    assert_eq!(reload(&kit, &payment).await.status, PaymentStatus::Failed);
    assert_eq!(
        order(&kit.services, payment.order_id).await.status,
        OrderStatus::Canceled
    );
    assert_eq!(balance(&kit.services, client_id).await, 649);
    inbox.apply_outcome(KASSA, &under).await.unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 649);
    let sent = texts(&jobs(&kit.services).await, client_id);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, PAYMENT_CREDITED_TEMPLATE);
    assert!(
        sent[0].1.contains("$6.49") && sent[0].1.contains("$10.00") && sent[0].1.contains("баланс"),
        "{}",
        sent[0].1
    );
    assert_eq!(
        audit_actions(&kit, &payment).await,
        vec!["payment.underpaid_credited".to_string()]
    );
    assert!(notices(&kit).await.is_empty());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn недоплата_в_другой_валюте_ждёт_ручного_зачисления_и_оповещает_админа() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 8102).await;
    let tariff = standard(&kit.services).await;
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let inbox = kit.services.inbox();
    let received = Amount::new(52_000, PayCurrency::Rub);
    let under = with(
        &payment,
        ProviderStatus::Underpaid { received },
        "u-2:wrong_amount",
    );
    inbox.apply_outcome(KASSA, &under).await.unwrap();
    inbox.apply_outcome(KASSA, &under).await.unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 0);
    let found = notices(&kit).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].kind, NoticeKind::ManualCredit);
    assert_eq!(found[0].payment_id, Some(payment.id));
    assert_eq!(found[0].client_id, Some(client_id));
    assert_eq!(found[0].amount, Some(received));
    assert!(found[0].text.contains("вручную"), "{}", found[0].text);
    let sent = texts(&jobs(&kit.services).await, client_id);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, PAYMENT_REVIEW_TEMPLATE);
    assert_eq!(
        audit_actions(&kit, &payment).await,
        vec!["payment.underpaid_review".to_string()]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn излишек_оплаты_выдаёт_заказ_и_зачисляется_на_баланс() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 8103).await;
    let tariff = standard(&kit.services).await;
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let inbox = kit.services.inbox();
    let over = with(
        &payment,
        ProviderStatus::Overpaid {
            received: usd(1_250),
        },
        "o-1:paid_over",
    );
    assert!(matches!(
        inbox.apply_outcome(KASSA, &over).await.unwrap(),
        AcceptResult::Accepted { .. }
    ));
    assert_eq!(reload(&kit, &payment).await.status, PaymentStatus::Paid);
    assert_eq!(
        order(&kit.services, payment.order_id).await.status,
        OrderStatus::Paid
    );
    assert_eq!(balance(&kit.services, client_id).await, 250);
    assert_eq!(
        inbox.apply_outcome(KASSA, &over).await.unwrap(),
        AcceptResult::AlreadyApplied {
            payment_id: payment.id
        }
    );
    assert_eq!(balance(&kit.services, client_id).await, 250);
    let queued = jobs(&kit.services).await;
    assert!(queued.iter().any(|job| matches!(
        job,
        JobPayload::FulfillOrder { order_id } if *order_id == payment.order_id
    )));
    let sent = texts(&queued, client_id);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, PAYMENT_CREDITED_TEMPLATE);
    assert!(sent[0].1.contains("$2.50"), "{}", sent[0].1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn проверка_aml_держит_платёж_и_отдаёт_заказ_на_ручную_проверку() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 8104).await;
    let tariff = standard(&kit.services).await;
    let soon = Utc::now() + chrono::Duration::minutes(30);
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), Some(soon)).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments()
            .attach_invoice(
                payment.id,
                &sn_store::InvoiceUpdate {
                    pay_url: Some("https://kassa.example.com/pay".into()),
                    provider_txid: Some("aml-1".into()),
                    expires_at: Some(soon),
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
    }
    let payment = reload(&kit, &payment).await;
    let inbox = kit.services.inbox();
    let locked = noticed(
        with(&payment, ProviderStatus::Held, "aml-1:locked"),
        NoticeKind::AmlCheck,
        "Heleket задержал оплату для проверки AML. Ссылка на анкету для клиента: https://aml.example.com/f/1",
    );
    inbox.apply_outcome(KASSA, &locked).await.unwrap();
    inbox.apply_outcome(KASSA, &locked).await.unwrap();
    let held = reload(&kit, &payment).await;
    assert_eq!(held.status, PaymentStatus::Pending);
    assert_eq!(held.expires_at, None);
    let order_now = order(&kit.services, payment.order_id).await;
    assert_eq!(order_now.status, OrderStatus::Review);
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        let reason = conn.orders().review_reason(order_now.id).await.unwrap();
        assert!(
            reason
                .as_deref()
                .unwrap_or_default()
                .contains("https://aml.example.com/f/1"),
            "{reason:?}"
        );
        let expired = conn
            .payments()
            .expire_stale(Utc::now() + chrono::Duration::days(3), 100)
            .await
            .unwrap();
        assert!(!expired.contains(&payment.id));
    }
    assert!(notices(&kit).await.is_empty());
    let sent = texts(&jobs(&kit.services).await, client_id);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, PAYMENT_REVIEW_TEMPLATE);
    let history = kit
        .services
        .clients()
        .payments(client_id, None, None)
        .await
        .unwrap();
    assert_eq!(history.items[0].pay_url, None);
    let refused = kit
        .services
        .fulfillment()
        .redeliver(order_now.id, Actor::System)
        .await;
    assert!(
        matches!(&refused, Err(ServiceError::Rejected { code, .. }) if *code == "order.awaiting_payment"),
        "{refused:?}"
    );
    assert!(matches!(
        kit.services.fulfillment().run(order_now.id).await.unwrap(),
        FulfillmentOutcome::Review { .. }
    ));
    let released = with(&payment, ProviderStatus::Paid, "aml-1:paid");
    assert!(matches!(
        inbox.apply_outcome(KASSA, &released).await.unwrap(),
        AcceptResult::Accepted { .. }
    ));
    assert_eq!(reload(&kit, &payment).await.status, PaymentStatus::Paid);
    assert_eq!(
        order(&kit.services, payment.order_id).await.status,
        OrderStatus::Paid
    );
    assert!(jobs(&kit.services).await.iter().any(|job| matches!(
        job,
        JobPayload::FulfillOrder { order_id } if *order_id == payment.order_id
    )));
    assert_eq!(balance(&kit.services, client_id).await, 0);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отказ_и_возврат_по_задержанной_оплате_отменяют_заказ() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 8105).await;
    let tariff = standard(&kit.services).await;
    let inbox = kit.services.inbox();
    let failed = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    inbox
        .apply_outcome(KASSA, &with(&failed, ProviderStatus::Held, "h-1:locked"))
        .await
        .unwrap();
    assert_eq!(
        order(&kit.services, failed.order_id).await.status,
        OrderStatus::Review
    );
    inbox
        .apply_outcome(KASSA, &with(&failed, ProviderStatus::Failed, "h-1:fail"))
        .await
        .unwrap();
    assert_eq!(reload(&kit, &failed).await.status, PaymentStatus::Failed);
    assert_eq!(
        order(&kit.services, failed.order_id).await.status,
        OrderStatus::Canceled
    );
    let refunded = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    inbox
        .apply_outcome(KASSA, &with(&refunded, ProviderStatus::Held, "h-2:locked"))
        .await
        .unwrap();
    let report = noticed(
        with(&refunded, ProviderStatus::Pending, "h-2:refund_paid"),
        NoticeKind::RefundReported,
        "Heleket сообщил о возврате денег по этому платежу",
    );
    inbox.apply_outcome(KASSA, &report).await.unwrap();
    assert_eq!(
        reload(&kit, &refunded).await.status,
        PaymentStatus::Canceled
    );
    assert_eq!(
        order(&kit.services, refunded.order_id).await.status,
        OrderStatus::Canceled
    );
    let found = notices(&kit).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].kind, NoticeKind::RefundReported);
    assert_eq!(found[0].payment_id, Some(refunded.id));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn сообщение_платёжки_о_возврате_по_оплаченному_платежу_не_меняет_учёт() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 8106).await;
    let tariff = standard(&kit.services).await;
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(KASSA, &with(&payment, ProviderStatus::Paid, "r-1:paid"))
        .await
        .unwrap();
    let report = noticed(
        with(&payment, ProviderStatus::Pending, "r-1:refund_paid"),
        NoticeKind::RefundReported,
        "Heleket сообщил о возврате денег по этому платежу",
    );
    inbox.apply_outcome(KASSA, &report).await.unwrap();
    inbox.apply_outcome(KASSA, &report).await.unwrap();
    assert_eq!(reload(&kit, &payment).await.status, PaymentStatus::Paid);
    let found = notices(&kit).await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].amount, Some(usd(1_000)));
    let dispute = noticed(
        WebhookOutcome {
            payment: None,
            amount: Some(usd(500)),
            ..with(&payment, ProviderStatus::Pending, "x-9:DISPUTED")
        },
        NoticeKind::Dispute,
        "Спор по неизвестному платежу",
    );
    assert!(matches!(
        inbox.apply_outcome(KASSA, &dispute).await.unwrap(),
        AcceptResult::Ignored { .. }
    ));
    let found = notices(&kit).await;
    assert_eq!(found.len(), 2);
    assert_eq!(found[1].kind, NoticeKind::Dispute);
    assert_eq!(found[1].payment_id, None);
    assert_eq!(found[1].client_id, None);
    assert_eq!(found[1].amount, Some(usd(500)));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn отозванное_разрешение_на_автосписание_снимает_сохранённую_карту() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 8107).await;
    let tariff = standard(&kit.services).await;
    let save = |token: &'static str| {
        let services = &kit.services;
        async move {
            let sealed = services
                .secrets()
                .unwrap()
                .seal_str(&saved_method_context(client_id, KASSA), token)
                .unwrap();
            let mut conn = services.store().conn().await.unwrap();
            conn.saved_methods()
                .save(client_id, KASSA, &sealed, Some("Карта *4477"))
                .await
                .unwrap();
        }
    };
    let active = || {
        let services = &kit.services;
        async move {
            let mut conn = services.store().conn().await.unwrap();
            conn.saved_methods().active(client_id, KASSA).await.unwrap()
        }
    };
    save("pm-old").await;
    let charge = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let poor = with(&charge, ProviderStatus::Canceled, "c-1:canceled");
    kit.services
        .inbox()
        .apply_outcome(KASSA, &poor)
        .await
        .unwrap();
    assert!(active().await.is_some());
    let revoked = WebhookOutcome {
        signal: Some(OutcomeSignal::MethodRevoked),
        ..with(&charge, ProviderStatus::Canceled, "c-1:revoked")
    };
    kit.services
        .inbox()
        .apply_outcome(KASSA, &revoked)
        .await
        .unwrap();
    assert!(active().await.is_none());
    assert!(
        audit_actions(&kit, &charge)
            .await
            .contains(&"payment.saved_method_revoked".to_string())
    );
    let older = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query("UPDATE payments SET created_at = now() - interval '1 day' WHERE id = $1")
            .bind(older.id.0)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let older = reload(&kit, &older).await;
    save("pm-new").await;
    let late = WebhookOutcome {
        signal: Some(OutcomeSignal::MethodRevoked),
        ..with(&older, ProviderStatus::Canceled, "c-2:revoked")
    };
    kit.services
        .inbox()
        .apply_outcome(KASSA, &late)
        .await
        .unwrap();
    assert!(active().await.is_some());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn одинаковое_тело_разных_типов_уведомлений_не_считается_повтором() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let client_id = client(&kit.services, 8108).await;
    let tariff = standard(&kit.services).await;
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let body = serde_json::json!({
        "event": "same", "payment": payment.public_id.to_string(), "txid": "k-1", "status": "waiting"
    });
    let mut kinds = Vec::new();
    for kind in [Some("check"), Some("pay"), Some("Pay"), None] {
        let mut hook = kassa_hook(KASSA_SECRET, &body);
        hook.request.route_suffix = kind.map(str::to_string);
        kit.services.inbox().handle(hook).await.unwrap();
        kinds.push(kind);
    }
    let keys: Vec<String> = inbox_rows(&kit)
        .await
        .into_iter()
        .map(|row| row.dedupe_key)
        .collect();
    assert_eq!(keys.len(), 3, "{keys:?}");
    let digest = digest_hex(body.to_string().as_bytes());
    assert_eq!(
        keys,
        vec![format!("check:{digest}"), format!("pay:{digest}"), digest]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn опрос_без_номера_транзакции_ищет_платёж_по_нашему_id() {
    let kassa = Arc::new(Kassa::default());
    *kassa.finds_by_payment.lock().unwrap() = true;
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let client_id = client(&kit.services, 8109).await;
    let tariff = standard(&kit.services).await;
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    assert_eq!(payment.provider_txid, None);
    let inbox = kit.services.inbox();
    *kassa.status.lock().unwrap() = Some(WebhookOutcome {
        provider_txid: Some("k-777".into()),
        ..with(&payment, ProviderStatus::Pending, "k-777:wait")
    });
    assert_eq!(
        inbox.poll_payment(payment.id).await.unwrap(),
        PollResult::Pending
    );
    assert_eq!(
        kassa.lookups.lock().unwrap().clone(),
        vec![payment.public_id.to_string()]
    );
    assert_eq!(reload(&kit, &payment).await.provider_txid, None);
    *kassa.status.lock().unwrap() = Some(WebhookOutcome {
        provider_txid: Some("k-777".into()),
        ..with(&payment, ProviderStatus::Paid, "k-777:paid")
    });
    assert_eq!(
        inbox.poll_payment(payment.id).await.unwrap(),
        PollResult::Settled
    );
    let paid = reload(&kit, &payment).await;
    assert_eq!(paid.status, PaymentStatus::Paid);
    assert_eq!(paid.provider_txid.as_deref(), Some("k-777"));
    assert_eq!(
        kassa.lookups.lock().unwrap().last().map(String::as_str),
        Some(payment.public_id.to_string().as_str())
    );
    let other = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    *kassa.finds_by_payment.lock().unwrap() = false;
    let before = kassa.lookups.lock().unwrap().len();
    assert_eq!(
        inbox.poll_payment(other.id).await.unwrap(),
        PollResult::Pending
    );
    assert_eq!(kassa.lookups.lock().unwrap().len(), before);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn возврат_вне_панели_записывается_без_обращения_к_платёжке() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 8110).await;
    let tariff = standard(&kit.services).await;
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(KASSA, &with(&payment, ProviderStatus::Paid, "out-1:paid"))
        .await
        .unwrap();
    let to_balance = RefundCommand {
        method: RefundMethod::Balance,
        outside: true,
        ..refund_command(Some(400), RefundSubscriptionAction::Keep, None)
    };
    assert!(matches!(
        inbox.refund(payment.id, &to_balance, admin).await,
        Err(ServiceError::Rejected { code, .. }) if code == "refund.outside_balance"
    ));
    let outside = RefundCommand {
        outside: true,
        ..refund_command(
            Some(400),
            RefundSubscriptionAction::Keep,
            Some("вернули в кабинете кассы"),
        )
    };
    let done = inbox.refund(payment.id, &outside, admin).await.unwrap();
    assert!(!done.pending);
    assert_eq!(done.status, PaymentStatus::PartiallyRefunded);
    assert!(kassa.refunds.lock().unwrap().is_empty());
    assert!(
        audit_actions(&kit, &payment)
            .await
            .contains(&"payment.refund_outside".to_string())
    );
    let reported = WebhookOutcome {
        event_key: "refund:lv-1".into(),
        ..outcome(
            &payment,
            ProviderStatus::PartiallyRefunded { refunded: usd(400) },
        )
    };
    assert_eq!(
        inbox.apply_outcome(KASSA, &reported).await.unwrap(),
        AcceptResult::AlreadyApplied {
            payment_id: payment.id
        }
    );
    let mut conn = kit.services.store().conn().await.unwrap();
    let refunds = conn.refunds().for_payment(payment.id).await.unwrap();
    assert_eq!(refunds.len(), 1);
    assert_eq!(
        refunds[0].provider_refund_id.as_deref(),
        Some("refund:lv-1")
    );
    assert_eq!(refunds[0].amount, usd(400));
    drop(conn);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повторное_нажатие_с_тем_же_ключом_не_возвращает_деньги_дважды() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 8111).await;
    let tariff = standard(&kit.services).await;
    let inbox = kit.services.inbox();
    let to_balance = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    inbox
        .apply_outcome(KASSA, &with(&to_balance, ProviderStatus::Paid, "k-1:paid"))
        .await
        .unwrap();
    let key = Uuid::new_v4();
    let command = RefundCommand {
        method: RefundMethod::Balance,
        key: Some(key),
        ..refund_command(Some(300), RefundSubscriptionAction::Keep, None)
    };
    let first = inbox.refund(to_balance.id, &command, admin).await.unwrap();
    let second = inbox.refund(to_balance.id, &command, admin).await.unwrap();
    assert_eq!(first.refund_id, second.refund_id);
    assert_eq!(second.amount, usd(300));
    assert_eq!(balance(&kit.services, client_id).await, 300);
    let full = RefundCommand {
        method: RefundMethod::Balance,
        key: Some(Uuid::new_v4()),
        ..refund_command(None, RefundSubscriptionAction::Keep, None)
    };
    let rest = inbox.refund(to_balance.id, &full, admin).await.unwrap();
    assert_eq!(rest.status, PaymentStatus::Refunded);
    let again = inbox.refund(to_balance.id, &full, admin).await.unwrap();
    assert_eq!(again.refund_id, rest.refund_id);
    assert_eq!(balance(&kit.services, client_id).await, 1_000);
    let via_kassa = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    inbox
        .apply_outcome(KASSA, &with(&via_kassa, ProviderStatus::Paid, "k-2:paid"))
        .await
        .unwrap();
    let key = Uuid::new_v4();
    let command = RefundCommand {
        key: Some(key),
        ..refund_command(Some(500), RefundSubscriptionAction::Keep, None)
    };
    let asked = inbox.refund(via_kassa.id, &command, admin).await.unwrap();
    let repeated = inbox.refund(via_kassa.id, &command, admin).await.unwrap();
    assert!(asked.pending && repeated.pending);
    assert_eq!(asked.request_id, repeated.request_id);
    assert_eq!(kassa.refunds.lock().unwrap().len(), 1);
    assert_eq!(kassa.refunds.lock().unwrap()[0].key, key);
    assert!(matches!(
        inbox.refund(to_balance.id, &command, admin).await,
        Err(ServiceError::Rejected { code, .. }) if code == "refund.key_reused"
    ));
    assert!(inbox.refund_keyed(via_kassa.id, key).await.unwrap());
    assert!(!inbox.refund_keyed(to_balance.id, key).await.unwrap());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn варианты_возврата_для_диалога_админки() {
    let kassa = Arc::new(Kassa::default());
    let Some(kit) = kassa_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 8112).await;
    let tariff = standard(&kit.services).await;
    let inbox = kit.services.inbox();
    let unpaid = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let options = inbox.refund_options(unpaid.id).await.unwrap();
    assert!(options.note.is_some());
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    inbox
        .apply_outcome(KASSA, &with(&payment, ProviderStatus::Paid, "opt-1:paid"))
        .await
        .unwrap();
    let options = inbox.refund_options(payment.id).await.unwrap();
    assert_eq!(options.note, None);
    assert_eq!(options.remaining, usd(1_000));
    assert_eq!(options.provider_title, "Касса с подтверждением возвратов");
    assert!(options.to_balance.available);
    assert!(options.via_provider.available);
    assert!(options.outside.available);
    assert!(!options.topup);
    inbox
        .refund(
            payment.id,
            &refund_command(Some(400), RefundSubscriptionAction::Keep, None),
            admin,
        )
        .await
        .unwrap();
    let options = inbox.refund_options(payment.id).await.unwrap();
    assert_eq!(options.waiting, usd(400));
    assert_eq!(options.remaining, usd(600));
    let topped = kassa_topup(&kit, client_id, 700).await;
    inbox
        .apply_outcome(KASSA, &outcome(&topped, ProviderStatus::Paid))
        .await
        .unwrap();
    let options = inbox.refund_options(topped.id).await.unwrap();
    assert!(options.topup);
    assert!(!options.to_balance.available);
    assert!(options.to_balance.note.is_some());
    let crypto = pending(
        &kit,
        client_id,
        &tariff,
        providers::CRYPTOPAY,
        usd(1_000),
        None,
    )
    .await;
    inbox
        .apply_outcome(
            providers::CRYPTOPAY,
            &with(&crypto, ProviderStatus::Paid, "cp-1:paid"),
        )
        .await
        .unwrap();
    let options = inbox.refund_options(crypto.id).await.unwrap();
    assert!(!options.via_provider.available);
    assert!(
        options
            .via_provider
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("Crypto Pay"),
        "{:?}",
        options.via_provider.note
    );
    assert!(options.outside.available);
    kit.db.cleanup().await;
}

const CP_SECRET: &str = "cp-api-secret";

async fn cloudpayments_kit() -> Option<Kit> {
    let kit = kassa_kit_with(
        Arc::new(Kassa::default()),
        Arc::new(SystemClock),
        vec![Arc::new(sn_payments::providers::CloudPayments::new())],
    )
    .await?;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::CLOUDPAYMENTS),
                &serde_json::json!({"enabled": true, "config": {"public_id": "pk_8f3b2c1d0e"}}),
                None,
            )
            .await
            .unwrap();
        settings::write_secret(
            &mut conn,
            kit.services.secrets(),
            &secret_keys::provider(providers::CLOUDPAYMENTS, "api_secret"),
            Some(CP_SECRET),
            None,
        )
        .await
        .unwrap();
    }
    kit.services.reload().await.unwrap();
    kit.services.set_remna(Some(kit.remna.clone()));
    Some(kit)
}

fn check_hook(form: &str) -> InboundWebhook {
    InboundWebhook {
        provider: providers::CLOUDPAYMENTS.into(),
        request: WebhookRequest {
            headers: vec![(
                "content-hmac".into(),
                sn_payments::providers::cloudpayments_signature(CP_SECRET, form.as_bytes()),
            )],
            body: Bytes::new(),
            remote_ip: "203.0.113.10".parse().ok(),
            route_suffix: Some("check".into()),
            query: Some(form.to_string()),
        },
    }
}

fn check_form(invoice: Uuid, account: Uuid, amount: &str) -> String {
    format!(
        "TransactionId=1504&Amount={amount}&Currency=RUB&PaymentAmount={amount}&PaymentCurrency=RUB&TestMode=0&Status=Completed&OperationType=Payment&InvoiceId={invoice}&AccountId={account}"
    )
}

async fn check_code(kit: &Kit, form: String) -> i64 {
    let reply = kit
        .services
        .inbox()
        .handle(check_hook(&form))
        .await
        .unwrap()
        .reply;
    match reply {
        WebhookReply::Json { body, .. } => body["code"].as_i64().unwrap(),
        other => panic!("ожидался JSON с кодом: {other:?}"),
    }
}

#[tokio::test]
async fn check_cloudpayments_сверяется_с_платежом_в_базе() {
    let Some(kit) = cloudpayments_kit().await else {
        return;
    };
    let client_id = client(&kit.services, 8113).await;
    let account = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.clients().get(client_id).await.unwrap().public_id
    };
    let tariff = standard(&kit.services).await;
    let rub = |minor| Amount::new(minor, PayCurrency::Rub);
    let open = pending(
        &kit,
        client_id,
        &tariff,
        providers::CLOUDPAYMENTS,
        rub(49_900),
        Some(Utc::now() + chrono::Duration::minutes(30)),
    )
    .await;
    assert_eq!(
        check_code(&kit, check_form(open.public_id, account, "499.00")).await,
        0
    );
    assert_eq!(
        check_code(&kit, check_form(Uuid::new_v4(), account, "499.00")).await,
        10
    );
    assert_eq!(
        check_code(&kit, check_form(open.public_id, Uuid::new_v4(), "499.00")).await,
        11
    );
    assert_eq!(
        check_code(&kit, check_form(open.public_id, account, "450.00")).await,
        12
    );
    let foreign = pending(&kit, client_id, &tariff, KASSA, rub(49_900), None).await;
    assert_eq!(
        check_code(&kit, check_form(foreign.public_id, account, "499.00")).await,
        10
    );
    let late = pending(
        &kit,
        client_id,
        &tariff,
        providers::CLOUDPAYMENTS,
        rub(49_900),
        Some(Utc::now() - chrono::Duration::minutes(1)),
    )
    .await;
    assert_eq!(
        check_code(&kit, check_form(late.public_id, account, "499.00")).await,
        20
    );
    kit.services
        .inbox()
        .apply_outcome(
            providers::CLOUDPAYMENTS,
            &with(&open, ProviderStatus::Paid, "cp-open:paid"),
        )
        .await
        .unwrap();
    assert_eq!(
        check_code(&kit, check_form(open.public_id, account, "499.00")).await,
        13
    );
    let refund = check_form(open.public_id, account, "100.00")
        .replace("OperationType=Payment", "OperationType=Refund");
    assert_eq!(check_code(&kit, refund).await, 0);
    let keys: Vec<String> = inbox_rows(&kit)
        .await
        .into_iter()
        .map(|row| row.dedupe_key)
        .collect();
    assert!(keys.iter().all(|key| key.starts_with("check:")), "{keys:?}");
    kit.db.cleanup().await;
}

#[tokio::test]
async fn оплата_другой_картой_после_отказа_первой_засчитывается() {
    for first in [ProviderStatus::Failed, ProviderStatus::Pending] {
        let kassa = Arc::new(Kassa::default());
        *kassa.finds_by_payment.lock().unwrap() = true;
        let Some(kit) = kassa_kit(kassa.clone()).await else {
            return;
        };
        let client_id = client(&kit.services, 8120).await;
        let tariff = standard(&kit.services).await;
        let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
        let inbox = kit.services.inbox();
        *kassa.status.lock().unwrap() = Some(WebhookOutcome {
            provider_txid: Some("k-1".into()),
            ..with(&payment, first, "k-1:first")
        });
        inbox.poll_payment(payment.id).await.unwrap();
        let second = inbox.apply_outcome(KASSA, &paid_by(&payment, "k-2")).await;
        assert!(second.is_ok(), "{first:?}: {second:?}");
        let after = reload(&kit, &payment).await;
        assert_eq!(after.status, PaymentStatus::Paid, "{first:?}");
        assert_eq!(after.provider_txid.as_deref(), Some("k-2"), "{first:?}");
        kit.db.cleanup().await;
    }
}

#[tokio::test]
async fn счёт_с_зачисленной_недоплатой_нельзя_отметить_оплаченным() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 8121).await;
    let tariff = standard(&kit.services).await;
    let payment = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    let inbox = kit.services.inbox();
    let under = with(
        &payment,
        ProviderStatus::Underpaid { received: usd(950) },
        "u-9:wrong_amount",
    );
    inbox.apply_outcome(KASSA, &under).await.unwrap();
    assert_eq!(balance(&kit.services, client_id).await, 950);
    match inbox
        .mark_paid(payment.id, admin, Some("клиент доплатил"))
        .await
    {
        Err(ServiceError::Rejected { code, message }) => {
            assert_eq!(code, "payment.underpaid_credited");
            assert!(message.contains("$9.50"), "{message}");
        }
        other => panic!("ожидали отказ: {other:?}"),
    }
    assert_eq!(reload(&kit, &payment).await.status, PaymentStatus::Failed);
    assert_eq!(
        order(&kit.services, payment.order_id).await.status,
        OrderStatus::Canceled
    );
    assert_eq!(balance(&kit.services, client_id).await, 950);
    let other = pending(&kit, client_id, &tariff, KASSA, usd(1_000), None).await;
    inbox.mark_paid(other.id, admin, None).await.unwrap();
    assert_eq!(reload(&kit, &other).await.status, PaymentStatus::Paid);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn токен_из_пути_уведомления_не_попадает_в_ключ_дублей() {
    let Some(kit) = kassa_kit(Arc::new(Kassa::default())).await else {
        return;
    };
    let body = serde_json::json!({ "event": "tok", "status": "waiting" });
    let token = "c2d9a17e4b6f40f3a0e85d1b7c3e9f21";
    let mut hook = kassa_hook(KASSA_SECRET, &body);
    hook.request.route_suffix = Some(token.into());
    kit.services.inbox().handle(hook).await.unwrap();
    let mut again = kassa_hook(KASSA_SECRET, &body);
    again.request.route_suffix = Some(token.into());
    kit.services.inbox().handle(again).await.unwrap();
    let keys: Vec<String> = inbox_rows(&kit)
        .await
        .into_iter()
        .map(|row| row.dedupe_key)
        .collect();
    assert_eq!(keys.len(), 1, "{keys:?}");
    assert!(!keys[0].contains(token), "{}", keys[0]);
    assert!(keys[0].starts_with("route-"), "{}", keys[0]);
    kit.db.cleanup().await;
}
