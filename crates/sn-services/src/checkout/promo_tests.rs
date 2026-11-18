use super::testkit::*;
use super::*;
use sn_domain::paging::PageRequest;
use sn_modules::catalog::promo::{self, ListFilter, NewPromo, NewPromoValue, PromoModule};
use sn_payments::{PaymentRef, ProviderStatus, WebhookOutcome, WebhookReply};

async fn promo_kit() -> Option<Kit> {
    kit_with(vec![Arc::new(PromoModule)]).await
}

fn percent(code: &str, percent: u8, max_uses: Option<u32>) -> NewPromo {
    NewPromo {
        code: code.into(),
        value: NewPromoValue::Percent { percent },
        max_uses,
        per_client: 1,
        valid_from: None,
        valid_until: None,
        first_purchase_only: false,
        tariff_ids: Vec::new(),
        note: None,
    }
}

async fn create_promo(kit: &Kit, new: NewPromo) {
    promo::create(&kit.services.module_context(), &new, None)
        .await
        .unwrap();
}

fn with_code(mut cart: CartRequest, code: &str) -> CartRequest {
    cart.promo_code = Some(code.into());
    cart
}

fn request(cart: CartRequest, provider: &str) -> CheckoutRequest {
    CheckoutRequest {
        cart,
        provider: provider.into(),
        method: None,
        channel: Channel::Cabinet,
        return_url: None,
    }
}

async fn counts(kit: &Kit) -> (u32, u32) {
    let page = promo::list(
        &kit.services.module_context(),
        ListFilter::All,
        PageRequest::default(),
    )
    .await
    .unwrap();
    (page.items[0].used, page.items[0].reserved)
}

async fn payment(kit: &Kit, public_id: Uuid) -> Payment {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.payments().by_public_id(public_id).await.unwrap()
}

async fn report(kit: &Kit, payment: &Payment, status: ProviderStatus) {
    kit.services
        .inbox()
        .apply_outcome(
            TEST_PAY,
            &WebhookOutcome {
                event_key: format!("{status:?}:{}", payment.id),
                payment: Some(PaymentRef::Payment(payment.id)),
                provider_txid: payment.provider_txid.clone(),
                status,
                amount: Some(payment.amount),
                saved_method: None,
                reply: WebhookReply::ok(),
                signal: None,
            },
        )
        .await
        .unwrap();
}

async fn orders_of(kit: &Kit, client_id: ClientId) -> Vec<Order> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.orders()
        .for_client(client_id, Default::default())
        .await
        .unwrap()
        .items
}

#[tokio::test]
async fn один_клиент_не_выставит_несколько_счетов_по_одноразовому_промокоду() {
    let Some(kit) = promo_kit().await else { return };
    let mut once = percent("ONCE50", 50, Some(1));
    once.first_purchase_only = true;
    create_promo(&kit, once).await;
    let client_id = client(&kit.services, 610).await;
    let tariff = standard(&kit.services).await;
    let cart = with_code(cart(&tariff, 30, None), "once50");
    let checkout = kit.services.checkout();
    let first = checkout
        .start(client_id, &request(cart.clone(), TEST_PAY))
        .await
        .unwrap();
    let CheckoutResult::Redirect { payment_id, .. } = first else {
        panic!("ожидалась ссылка на оплату: {first:?}");
    };
    assert_eq!(counts(&kit).await, (0, 1));
    for _ in 0..2 {
        let err = checkout
            .start(client_id, &request(cart.clone(), TEST_PAY))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("неоплаченному счёту"), "{err}");
    }
    assert_eq!(kit.pay.invoices.lock().unwrap().len(), 1);
    assert_eq!(orders_of(&kit, client_id).await.len(), 1);
    let paid = payment(&kit, payment_id).await;
    assert_eq!(paid.amount, Amount::new(149, PayCurrency::Usd));
    report(&kit, &paid, ProviderStatus::Paid).await;
    assert_eq!(counts(&kit).await, (1, 0));
    let again = checkout
        .start(client_id, &request(cart, TEST_PAY))
        .await
        .unwrap_err();
    assert!(again.to_string().contains("уже использовали"), "{again}");
    assert_eq!(orders_of(&kit, client_id).await.len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn параллельные_покупки_с_баланса_не_превышают_лимит_промокода() {
    let Some(kit) = promo_kit().await else { return };
    create_promo(&kit, percent("FIRST1", 50, Some(1))).await;
    let tariff = standard(&kit.services).await;
    let first = client(&kit.services, 611).await;
    let second = client(&kit.services, 612).await;
    credit(&kit.services, first, 1_000).await;
    credit(&kit.services, second, 1_000).await;
    let cart = with_code(cart(&tariff, 30, None), "FIRST1");
    let checkout = kit.services.checkout();
    let first_request = request(cart.clone(), providers::BALANCE);
    let second_request = request(cart, providers::BALANCE);
    let (a, b) = tokio::join!(
        checkout.start(first, &first_request),
        checkout.start(second, &second_request),
    );
    let completed = [&a, &b]
        .iter()
        .filter(|r| matches!(r, Ok(CheckoutResult::Completed { .. })))
        .count();
    assert_eq!(completed, 1, "{a:?} / {b:?}");
    assert_eq!(
        counts(&kit).await,
        (1, 0),
        "использование записано в той же транзакции, что и заказ"
    );
    let mut balances = [
        balance(&kit.services, first).await,
        balance(&kit.services, second).await,
    ];
    balances.sort_unstable();
    assert_eq!(balances, [851, 1_000]);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn бесплатный_заказ_по_промокоду_не_оформляется_дважды_параллельно() {
    let Some(kit) = promo_kit().await else { return };
    create_promo(&kit, percent("GIFT100", 100, None)).await;
    let tariff = standard(&kit.services).await;
    let client_id = client(&kit.services, 613).await;
    let cart = with_code(cart(&tariff, 30, None), "GIFT100");
    let checkout = kit.services.checkout();
    let first_request = request(cart.clone(), TEST_PAY);
    let second_request = request(cart, TEST_PAY);
    let (a, b) = tokio::join!(
        checkout.start(client_id, &first_request),
        checkout.start(client_id, &second_request),
    );
    let completed = [&a, &b]
        .iter()
        .filter(|r| matches!(r, Ok(CheckoutResult::Completed { .. })))
        .count();
    assert_eq!(completed, 1, "{a:?} / {b:?}");
    assert_eq!(counts(&kit).await, (1, 0));
    assert_eq!(orders_of(&kit, client_id).await.len(), 1);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn неоплаченный_счёт_держит_промокод_до_отмены_или_истечения() {
    let Some(kit) = promo_kit().await else { return };
    create_promo(&kit, percent("LAST", 10, Some(1))).await;
    let tariff = standard(&kit.services).await;
    let first = client(&kit.services, 614).await;
    let second = client(&kit.services, 615).await;
    let third = client(&kit.services, 616).await;
    let cart = with_code(cart(&tariff, 30, None), "LAST");
    let checkout = kit.services.checkout();
    let CheckoutResult::Redirect { payment_id, .. } = checkout
        .start(first, &request(cart.clone(), TEST_PAY))
        .await
        .unwrap()
    else {
        panic!("ожидалась ссылка на оплату");
    };
    let busy = checkout.quote(second, &cart).await.unwrap_err();
    assert!(busy.to_string().contains("не подходит"), "{busy}");
    let failed = payment(&kit, payment_id).await;
    report(&kit, &failed, ProviderStatus::Failed).await;
    assert_eq!(counts(&kit).await, (0, 0));
    let CheckoutResult::Redirect { payment_id, .. } = checkout
        .start(second, &request(cart.clone(), TEST_PAY))
        .await
        .unwrap()
    else {
        panic!("ожидалась ссылка на оплату");
    };
    assert!(checkout.quote(third, &cart).await.is_err());
    let stale = payment(&kit, payment_id).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query("UPDATE payments SET expires_at = now() - interval '1 minute' WHERE id = $1")
            .bind(stale.id.0)
            .execute(conn.conn())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE promo_redemptions SET reserved_until = now() - interval '1 minute' WHERE order_id = $1",
        )
        .bind(stale.order_id.0)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    assert_eq!(counts(&kit).await, (0, 0));
    assert!(matches!(
        checkout
            .start(third, &request(cart, TEST_PAY))
            .await
            .unwrap(),
        CheckoutResult::Redirect { .. }
    ));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn поздняя_оплата_сверх_лимита_промокода_выдаётся_по_цене_счёта() {
    let Some(kit) = promo_kit().await else { return };
    create_promo(&kit, percent("LATE", 10, Some(1))).await;
    let tariff = standard(&kit.services).await;
    let late = client(&kit.services, 619).await;
    let prompt = client(&kit.services, 620).await;
    credit(&kit.services, prompt, 1_000).await;
    let cart = with_code(cart(&tariff, 30, None), "LATE");
    let checkout = kit.services.checkout();
    let CheckoutResult::Redirect { payment_id, .. } = checkout
        .start(late, &request(cart.clone(), TEST_PAY))
        .await
        .unwrap()
    else {
        panic!("ожидалась ссылка на оплату");
    };
    let stale = payment(&kit, payment_id).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query("UPDATE payments SET expires_at = now() - interval '1 minute' WHERE id = $1")
            .bind(stale.id.0)
            .execute(conn.conn())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE promo_redemptions SET reserved_until = now() - interval '1 minute' WHERE order_id = $1",
        )
        .bind(stale.order_id.0)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    assert!(matches!(
        checkout
            .start(prompt, &request(cart, providers::BALANCE))
            .await
            .unwrap(),
        CheckoutResult::Completed { .. }
    ));
    assert_eq!(counts(&kit).await, (1, 0));
    report(&kit, &stale, ProviderStatus::Paid).await;
    assert_eq!(
        payment(&kit, payment_id).await.status,
        PaymentStatus::Paid,
        "деньги приняты"
    );
    let held = order(&kit.services, stale.order_id).await;
    assert_ne!(held.status, OrderStatus::Review);
    assert!(
        jobs(&kit.services)
            .await
            .contains(&JobPayload::FulfillOrder {
                order_id: stale.order_id
            }),
        "счёт оплачен по цене со скидкой и выдаётся без ручной проверки"
    );
    assert_eq!(counts(&kit).await, (2, 0));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn оплата_после_отказа_кассы_выдаётся_по_цене_счёта() {
    let Some(kit) = promo_kit().await else { return };
    create_promo(&kit, percent("EDGE", 10, Some(1))).await;
    let tariff = standard(&kit.services).await;
    let declined = client(&kit.services, 621).await;
    let other = client(&kit.services, 622).await;
    credit(&kit.services, other, 1_000).await;
    let cart = with_code(cart(&tariff, 30, None), "EDGE");
    let checkout = kit.services.checkout();
    let CheckoutResult::Redirect { payment_id, .. } = checkout
        .start(declined, &request(cart.clone(), TEST_PAY))
        .await
        .unwrap()
    else {
        panic!("ожидалась ссылка на оплату");
    };
    let first = payment(&kit, payment_id).await;
    report(&kit, &first, ProviderStatus::Failed).await;
    assert_eq!(
        order(&kit.services, first.order_id).await.status,
        OrderStatus::Canceled
    );
    assert_eq!(counts(&kit).await, (0, 0));
    assert!(matches!(
        checkout
            .start(other, &request(cart, providers::BALANCE))
            .await
            .unwrap(),
        CheckoutResult::Completed { .. }
    ));
    report(&kit, &first, ProviderStatus::Paid).await;
    assert_ne!(
        order(&kit.services, first.order_id).await.status,
        OrderStatus::Review,
        "поздняя оплата выдаётся по цене счёта"
    );
    assert!(
        jobs(&kit.services)
            .await
            .contains(&JobPayload::FulfillOrder {
                order_id: first.order_id
            })
    );
    assert_eq!(counts(&kit).await, (2, 0));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn блокировка_клиента_снимает_резерв_промокода() {
    let Some(kit) = promo_kit().await else { return };
    create_promo(&kit, percent("HOLD", 10, Some(1))).await;
    let tariff = standard(&kit.services).await;
    let blocked = client(&kit.services, 623).await;
    let cart = with_code(cart(&tariff, 30, None), "HOLD");
    assert!(matches!(
        kit.services
            .checkout()
            .start(blocked, &request(cart, TEST_PAY))
            .await
            .unwrap(),
        CheckoutResult::Redirect { .. }
    ));
    assert_eq!(counts(&kit).await, (0, 1));
    kit.services
        .clients()
        .block(
            blocked,
            Some("подозрение на мошенничество"),
            crate::Actor::System,
        )
        .await
        .unwrap();
    let status: String = {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query_scalar("SELECT status FROM promo_redemptions")
            .fetch_one(conn.conn())
            .await
            .unwrap()
    };
    assert_eq!(status, "released");
    assert_eq!(counts(&kit).await, (0, 0));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn сбой_кассы_сразу_освобождает_промокод() {
    let Some(kit) = promo_kit().await else { return };
    create_promo(&kit, percent("SOLO", 10, Some(1))).await;
    let tariff = standard(&kit.services).await;
    let first = client(&kit.services, 617).await;
    let second = client(&kit.services, 618).await;
    let cart = with_code(cart(&tariff, 30, None), "SOLO");
    let checkout = kit.services.checkout();
    *kit.pay.fail_invoice.lock().unwrap() = true;
    let err = checkout
        .start(first, &request(cart.clone(), TEST_PAY))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Payment(_)), "{err:?}");
    assert_eq!(counts(&kit).await, (0, 0));
    let status: String = {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query_scalar("SELECT status FROM promo_redemptions")
            .fetch_one(conn.conn())
            .await
            .unwrap()
    };
    assert_eq!(status, "released");
    *kit.pay.fail_invoice.lock().unwrap() = false;
    assert!(matches!(
        checkout
            .start(second, &request(cart, TEST_PAY))
            .await
            .unwrap(),
        CheckoutResult::Redirect { .. }
    ));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn расчёт_и_оформление_считают_неудачные_промокоды_по_адресу() {
    let Some(kit) = promo_kit().await else { return };
    create_promo(&kit, percent("REAL20", 20, None)).await;
    let tariff = standard(&kit.services).await;
    let office: IpAddr = "203.0.113.40".parse().unwrap();
    let neighbour: IpAddr = "203.0.113.41".parse().unwrap();
    let checkout = kit.services.checkout();
    let accounts = promo::ADDRESS_FAILURE_LIMIT / promo::FAILURE_LIMIT;
    for account in 0..accounts {
        let who = client(&kit.services, 630 + i64::from(account)).await;
        for n in 0..promo::FAILURE_LIMIT {
            let wrong = with_code(cart(&tariff, 30, None), &format!("GUESS{account}X{n}"));
            let err = checkout
                .quote_from(who, &wrong, Some(office))
                .await
                .unwrap_err();
            assert!(err.to_string().contains("нет или он не подходит"), "{err}");
        }
    }
    let fresh = client(&kit.services, 640).await;
    let cart = with_code(cart(&tariff, 30, None), "REAL20");
    let limited = checkout
        .quote_from(fresh, &cart, Some(office))
        .await
        .unwrap_err();
    assert!(limited.to_string().contains("Слишком много"), "{limited}");
    let limited = checkout
        .start_from(fresh, &request(cart.clone(), TEST_PAY), Some(office))
        .await
        .unwrap_err();
    assert!(limited.to_string().contains("Слишком много"), "{limited}");
    assert!(kit.pay.invoices.lock().unwrap().is_empty());
    assert!(orders_of(&kit, fresh).await.is_empty());
    let view = checkout
        .quote_from(fresh, &cart, Some(neighbour))
        .await
        .unwrap();
    assert_eq!(view.quote.total.minor, 239);
    assert!(
        checkout.quote(fresh, &cart).await.is_ok(),
        "без адреса действует только лимит клиента"
    );
    assert!(matches!(
        checkout
            .start_from(fresh, &request(cart, TEST_PAY), Some(neighbour))
            .await
            .unwrap(),
        CheckoutResult::Redirect { .. }
    ));
    kit.db.cleanup().await;
}
