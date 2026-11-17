use super::*;
use crate::catalog::testing::{TestCore, client, context, order, paid_payment, usd};
use crate::registry::ModuleRegistry;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::TimeZone;
use sn_domain::access::{AdminPrincipal, AdminRole, ClientChannel, ClientPrincipal};
use sn_domain::ids::{CategoryId, PeriodId, RemnaUserId};
use sn_domain::money::{Amount, Stars};
use sn_domain::order::OrderKind;
use sn_domain::pricing::{QuoteRequest, quote};
use sn_domain::subscription::{AutoRenewState, RemnaLink, SubscriptionKind};
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, Tariff, TariffPeriod};
use sn_domain::traffic::TrafficPolicy;
use sn_store::NewSubscription;
use sn_store::testing::test_store;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tower::ServiceExt;

fn code(kind: PromoKind) -> PromoCode {
    PromoCode {
        id: 1,
        code: "ZIMA".into(),
        kind,
        max_uses: Some(10),
        per_client: 1,
        valid_from: None,
        valid_until: None,
        first_purchase_only: false,
        tariff_ids: Vec::new(),
        active: true,
        note: None,
        created_at: now(),
    }
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 0).unwrap()
}

fn checkout() -> Purpose {
    Purpose::Checkout {
        tariff: TariffId(1),
        currency: Currency::Usd,
    }
}

fn tariff() -> Tariff {
    Tariff {
        id: TariffId(1),
        category_id: CategoryId(1),
        name: "Стандарт".into(),
        description: None,
        on_sale: true,
        sort: 0,
        periods: vec![TariffPeriod {
            id: PeriodId(12),
            days: 90,
            prices: CurrencyPrices {
                usd: Some(699),
                rub: Some(49_900),
            },
            stars: Some(Stars(350)),
            sort: 0,
            active: true,
        }],
        devices: DevicePolicy {
            included: 3,
            extra: None,
        },
        traffic: TrafficPolicy::UNLIMITED,
        squads: Vec::new(),
        external_squad: None,
    }
}

fn priced(adjustments: &[PriceAdjustment]) -> Quote {
    quote(&QuoteRequest {
        currency: Currency::Usd,
        tariff: &tariff(),
        period_id: PeriodId(12),
        extra_devices: 0,
        sell_extra_devices: false,
        adjustments,
    })
    .unwrap()
}

fn intent() -> PurchaseIntent {
    PurchaseIntent::Subscription {
        tariff_id: TariffId(1),
        period_id: PeriodId(12),
        extra_devices: 0,
        target: None,
    }
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

fn hook(client_id: ClientId, code: &str) -> QuoteHook {
    QuoteHook {
        client_id,
        tariff_id: TariffId(1),
        period_id: PeriodId(12),
        order_kind: OrderKind::New,
        currency: Currency::Usd,
        promo_code: Some(code.into()),
    }
}

#[test]
fn код_приводится_к_верхнему_регистру_без_лишнего() {
    assert_eq!(normalize_code(" zima ").as_deref(), Some("ZIMA"));
    assert_eq!(
        normalize_code("Winter-2026_x").as_deref(),
        Some("WINTER-2026_X")
    );
    assert_eq!(normalize_code("ЗИМА"), None);
    assert_eq!(normalize_code("AB"), None);
    assert_eq!(normalize_code("%ZIMA%"), None);
    assert_eq!(normalize_code(&"A".repeat(33)), None);
}

#[test]
fn проверка_промокода_по_правилам() {
    let usage = Usage::default();
    let base = code(PromoKind::Percent { percent: 10 });
    assert_eq!(evaluate(&base, &usage, now(), checkout()), Ok(()));
    let mut disabled = base.clone();
    disabled.active = false;
    assert_eq!(
        evaluate(&disabled, &usage, now(), checkout()),
        Err(PromoRejection::Disabled)
    );
    let mut later = base.clone();
    later.valid_from = Some(now() + Duration::days(1));
    assert_eq!(
        evaluate(&later, &usage, now(), checkout()),
        Err(PromoRejection::NotStarted)
    );
    let mut expired = base.clone();
    expired.valid_until = Some(now());
    assert_eq!(
        evaluate(&expired, &usage, now(), checkout()),
        Err(PromoRejection::Expired)
    );
    let exhausted = Usage {
        active_total: 10,
        ..Usage::default()
    };
    assert_eq!(
        evaluate(&base, &exhausted, now(), checkout()),
        Err(PromoRejection::Exhausted)
    );
    let mine = Usage {
        active_total: 1,
        client_active: 1,
        client_has_purchases: false,
        ..Usage::default()
    };
    assert_eq!(
        evaluate(&base, &mine, now(), checkout()),
        Err(PromoRejection::AlreadyUsed)
    );
    let unpaid = Usage {
        active_total: 1,
        client_active: 1,
        client_held: 1,
        client_has_purchases: false,
    };
    assert_eq!(
        evaluate(&base, &unpaid, now(), checkout()),
        Err(PromoRejection::Reserved)
    );
    let mut first = base.clone();
    first.first_purchase_only = true;
    let buyer = Usage {
        client_has_purchases: true,
        ..Usage::default()
    };
    assert_eq!(
        evaluate(&first, &buyer, now(), checkout()),
        Err(PromoRejection::FirstPurchaseOnly)
    );
    let mut scoped = base.clone();
    scoped.tariff_ids = vec![TariffId(7)];
    assert_eq!(
        evaluate(&scoped, &usage, now(), checkout()),
        Err(PromoRejection::WrongTariff)
    );
    assert_eq!(
        evaluate(
            &scoped,
            &usage,
            now(),
            Purpose::Check {
                tariff: None,
                currency: Currency::Usd
            }
        ),
        Ok(())
    );
}

#[test]
fn фиксированная_скидка_только_в_своей_валюте_а_дни_отдельно() {
    let usage = Usage::default();
    let fixed = code(PromoKind::Fixed {
        amount: Money::new(10_000, Currency::Rub),
    });
    assert_eq!(
        evaluate(&fixed, &usage, now(), checkout()),
        Err(PromoRejection::WrongCurrency)
    );
    let days = code(PromoKind::Days { days: 7 });
    assert_eq!(
        evaluate(&days, &usage, now(), checkout()),
        Err(PromoRejection::DaysCode)
    );
    assert_eq!(evaluate(&days, &usage, now(), Purpose::Activate), Ok(()));
    let discount = code(PromoKind::Percent { percent: 5 });
    assert_eq!(
        evaluate(&discount, &usage, now(), Purpose::Activate),
        Err(PromoRejection::DiscountCode)
    );
    assert_eq!(days.adjustment(), None);
    assert_eq!(discount.label(), "Промокод ZIMA, −5%");
}

#[test]
fn новый_промокод_проверяется_до_записи() {
    let mut new = percent("zima", 10, Some(100));
    assert_eq!(
        validate_new(&new, Currency::Usd).unwrap(),
        ("ZIMA".to_string(), PromoKind::Percent { percent: 10 })
    );
    new.value = NewPromoValue::Percent { percent: 0 };
    assert!(validate_new(&new, Currency::Usd).is_err());
    new.value = NewPromoValue::Fixed {
        amount: "1.50".into(),
    };
    assert_eq!(
        validate_new(&new, Currency::Usd).unwrap().1,
        PromoKind::Fixed { amount: usd(150) }
    );
    assert!(validate_new(&new, Currency::Rub).is_err());
    new.value = NewPromoValue::Fixed {
        amount: "1.505".into(),
    };
    assert!(validate_new(&new, Currency::Usd).is_err());
    new.value = NewPromoValue::Days { days: 7 };
    new.tariff_ids = vec![TariffId(1)];
    assert!(validate_new(&new, Currency::Usd).is_err());
    new.tariff_ids.clear();
    new.max_uses = Some(0);
    assert!(validate_new(&new, Currency::Usd).is_err());
}

#[test]
fn тело_запроса_на_создание_разбирается() {
    let parsed: NewPromo = serde_json::from_value(serde_json::json!({
        "code": "ZIMA", "kind": "percent", "percent": 10, "max_uses": 100
    }))
    .unwrap();
    assert_eq!(parsed.value, NewPromoValue::Percent { percent: 10 });
    assert_eq!(parsed.per_client, 1);
    assert_eq!(parsed.max_uses, Some(100));
}

#[tokio::test]
async fn скидка_по_точному_коду_попадает_в_расчёт() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    create(&ctx, &percent("ZIMA2026", 10, None), None)
        .await
        .unwrap();
    let buyer = client(&db.store).await;
    let adjustments = PromoModule
        .quote_adjustments(&ctx, &hook(buyer, " zima2026 "))
        .await
        .unwrap();
    assert_eq!(adjustments.len(), 1);
    let q = priced(&adjustments);
    assert_eq!(q.total, usd(629));
    assert_eq!(q.stars.unwrap().total, Stars(315));
    assert_eq!(
        promo_in_quote(&q).map(|(_, c)| c).as_deref(),
        Some("ZIMA2026")
    );
    for wrong in ["ZIMA", "ZIMA202", "ZIMA20266", "%ZIMA%"] {
        assert!(
            matches!(
                PromoModule
                    .quote_adjustments(&ctx, &hook(buyer, wrong))
                    .await,
                Err(ModuleError::Invalid(_))
            ),
            "{wrong}"
        );
    }
    let none = PromoModule
        .quote_adjustments(
            &ctx,
            &QuoteHook {
                promo_code: Some("  ".into()),
                ..hook(buyer, "")
            },
        )
        .await
        .unwrap();
    assert!(none.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn последний_промокод_резервирует_только_один_счёт() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let promo = create(&ctx, &percent("LAST", 10, Some(1)), None)
        .await
        .unwrap();
    let adjustment = promo.adjustment().unwrap();
    let q = priced(std::slice::from_ref(&adjustment));
    let first = client(&db.store).await;
    let second = client(&db.store).await;
    let a = order(&db.store, first, OrderKind::New, intent(), q.clone()).await;
    let b = order(&db.store, second, OrderKind::New, intent(), q.clone()).await;
    let until = Utc::now() + Duration::minutes(60);
    let (ra, rb) = tokio::join!(reserve(&ctx, &a, until), reserve(&ctx, &b, until));
    let results = [ra, rb];
    let reserved = results.iter().filter(|r| matches!(r, Ok(Some(_)))).count();
    let rejected = results
        .iter()
        .filter(|r| matches!(r, Err(ModuleError::Invalid(_))))
        .count();
    assert_eq!((reserved, rejected), (1, 1));
    let winner = if matches!(results[0], Ok(Some(_))) {
        &a
    } else {
        &b
    };
    assert!(matches!(reserve(&ctx, winner, until).await, Ok(Some(_))));
    let third = client(&db.store).await;
    assert!(matches!(
        check(
            &ctx,
            third,
            "last",
            Purpose::Check {
                tariff: None,
                currency: Currency::Usd
            }
        )
        .await,
        Err(PromoError::Rejected(PromoRejection::Exhausted))
    ));
    assert!(release(&ctx, winner.id).await.unwrap());
    assert!(
        check(
            &ctx,
            third,
            "last",
            Purpose::Check {
                tariff: None,
                currency: Currency::Usd
            }
        )
        .await
        .is_ok()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn просроченный_резерв_не_держит_промокод() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let promo = create(&ctx, &percent("ONCE", 15, Some(1)), None)
        .await
        .unwrap();
    let q = priced(&[promo.adjustment().unwrap()]);
    let buyer = client(&db.store).await;
    let late = order(&db.store, buyer, OrderKind::New, intent(), q.clone()).await;
    reserve(&ctx, &late, Utc::now() - Duration::minutes(1))
        .await
        .unwrap();
    let other = client(&db.store).await;
    let fresh = order(&db.store, other, OrderKind::New, intent(), q).await;
    assert!(matches!(
        reserve(&ctx, &fresh, Utc::now() + Duration::minutes(30)).await,
        Ok(Some(_))
    ));
    db.cleanup().await;
}

#[tokio::test]
async fn оплата_закрепляет_использование_а_возврат_освобождает() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let promo = create(&ctx, &percent("PAIDONE", 10, Some(1)), None)
        .await
        .unwrap();
    let q = priced(&[promo.adjustment().unwrap()]);
    let buyer = client(&db.store).await;
    let paid = order(&db.store, buyer, OrderKind::New, intent(), q.clone()).await;
    let payment = paid_payment(
        &db.store,
        paid.id,
        buyer,
        "cryptopay",
        Amount::from(q.total),
    )
    .await;
    let event = DomainEvent::PaymentPaid {
        payment_id: payment.id,
        order_id: paid.id,
        client_id: buyer,
        order_kind: OrderKind::New,
        amount: payment.amount,
        provider: payment.provider.clone(),
    };
    PromoModule.on_event(&ctx, &event).await.unwrap();
    PromoModule.on_event(&ctx, &event).await.unwrap();
    let page = list(&ctx, ListFilter::All, PageRequest::default())
        .await
        .unwrap();
    assert_eq!((page.items[0].used, page.items[0].reserved), (1, 0));
    let other = client(&db.store).await;
    assert!(matches!(
        check(
            &ctx,
            other,
            "PAIDONE",
            Purpose::Check {
                tariff: None,
                currency: Currency::Usd
            }
        )
        .await,
        Err(PromoError::Rejected(PromoRejection::Exhausted))
    ));
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.payments()
            .transition(payment.id, PaymentStatus::Refunded, None, None)
            .await
            .unwrap();
    }
    PromoModule
        .on_event(
            &ctx,
            &DomainEvent::PaymentRefunded {
                payment_id: payment.id,
                order_id: paid.id,
                client_id: buyer,
                amount: payment.amount,
            },
        )
        .await
        .unwrap();
    assert!(
        check(
            &ctx,
            other,
            "PAIDONE",
            Purpose::Check {
                tariff: None,
                currency: Currency::Usd
            }
        )
        .await
        .is_ok()
    );
    db.cleanup().await;
}

async fn subscription(db: &sn_store::testing::TestDb, owner: ClientId) -> SubscriptionId {
    let mut conn = db.store.conn().await.unwrap();
    let sub = conn
        .subscriptions()
        .create(&NewSubscription {
            client_id: owner,
            tariff_id: None,
            kind: SubscriptionKind::Paid,
            extra_devices: 0,
            autorenew: AutoRenewState::default(),
        })
        .await
        .unwrap();
    conn.subscriptions()
        .link_remna(
            sub.id,
            &RemnaLink {
                user_id: RemnaUserId(sub.id.0 + 1000),
                username: format!("sn_test_{}", sub.id),
                short_uuid: None,
            },
        )
        .await
        .unwrap();
    sub.id
}

#[tokio::test]
async fn промокод_на_дни_добавляет_дни_через_ядро_один_раз() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut days = percent("WEEK", 1, None);
    days.value = NewPromoValue::Days { days: 7 };
    create(&ctx, &days, None).await.unwrap();
    create(&ctx, &percent("SALE", 10, None), None)
        .await
        .unwrap();
    let owner = client(&db.store).await;
    let stranger = client(&db.store).await;
    let sub = subscription(&db, owner).await;
    assert!(matches!(
        activate(&ctx, stranger, "WEEK", sub).await,
        Err(PromoError::Rejected(PromoRejection::NoSubscription))
    ));
    assert!(matches!(
        activate(&ctx, owner, "SALE", sub).await,
        Err(PromoError::Rejected(PromoRejection::DiscountCode))
    ));
    core.fail_days.store(true, Ordering::SeqCst);
    assert!(matches!(
        activate(&ctx, owner, "week", sub).await,
        Err(PromoError::Module(ModuleError::Core(_)))
    ));
    core.fail_days.store(false, Ordering::SeqCst);
    let done = activate(&ctx, owner, "week", sub).await.unwrap();
    assert_eq!((done.days, done.subscription_id), (7, sub));
    assert_eq!(core.days(), vec![(sub, 7, "промокод WEEK".to_string())]);
    assert!(matches!(
        activate(&ctx, owner, "WEEK", sub).await,
        Err(PromoError::Rejected(PromoRejection::AlreadyUsed))
    ));
    db.cleanup().await;
}

fn admin_as(admin_id: sn_domain::ids::AdminId, role: AdminRole) -> AdminPrincipal {
    AdminPrincipal {
        admin_id,
        role,
        permissions: Vec::new(),
        session_id: 1,
    }
}

async fn admin_id(store: &sn_store::Store) -> sn_domain::ids::AdminId {
    let mut conn = store.conn().await.unwrap();
    conn.admins()
        .create(&sn_store::NewAdmin {
            email: "owner@example.com".into(),
            password_hash: "x".into(),
            role: AdminRole::Owner,
            must_change_password: false,
        })
        .await
        .unwrap()
        .id
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn админка_создаёт_и_выключает_промокод() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.admin_router(ctx.clone());
    let owner = admin_id(&db.store).await;
    let admin = |role: AdminRole| admin_as(owner, role);
    let create_request = |role: AdminRole| {
        Request::post("/promo/codes")
            .header("content-type", "application/json")
            .extension(admin(role))
            .body(Body::from(
                serde_json::json!({"code": "spring", "kind": "fixed", "amount": "1.00", "max_uses": 50})
                    .to_string(),
            ))
            .unwrap()
    };
    let denied = router
        .clone()
        .oneshot(create_request(AdminRole::Support))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let created = router
        .clone()
        .oneshot(create_request(AdminRole::Admin))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = body_json(created).await;
    assert_eq!(created["code"], "SPRING");
    assert_eq!(created["kind"]["type"], "fixed");
    let duplicate = router
        .clone()
        .oneshot(create_request(AdminRole::Admin))
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let id = created["id"].as_i64().unwrap();
    let disabled = router
        .clone()
        .oneshot(
            Request::post(format!("/promo/codes/{id}/disable"))
                .extension(admin(AdminRole::Owner))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disabled.status(), StatusCode::OK);
    let listed = router
        .oneshot(
            Request::get("/promo/codes?status=disabled")
                .extension(admin(AdminRole::Owner))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let listed = body_json(listed).await;
    assert_eq!(listed["items"][0]["code"], "SPRING");
    assert_eq!(listed["items"][0]["active"], false);
    db.cleanup().await;
}

#[tokio::test]
async fn клиент_проверяет_код_и_видит_причину_отказа() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    create(&ctx, &percent("FRIEND", 20, None), None)
        .await
        .unwrap();
    let buyer = client(&db.store).await;
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx);
    let principal = ClientPrincipal {
        client_id: buyer,
        channel: ClientChannel::Cabinet,
        full_access: true,
        session_id: Some(1),
    };
    let ask = |code: &str| {
        Request::post("/promo/check")
            .header("content-type", "application/json")
            .extension(principal.clone())
            .body(Body::from(serde_json::json!({ "code": code }).to_string()))
            .unwrap()
    };
    let ok = router.clone().oneshot(ask("friend")).await.unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let ok = body_json(ok).await;
    assert_eq!(ok["label"], "Промокод FRIEND, −20%");
    assert_eq!(ok["discount"], true);
    let missing = router.oneshot(ask("FRIENDS")).await.unwrap();
    assert_eq!(missing.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let missing = body_json(missing).await;
    assert_eq!(missing["error"]["code"], "promo.not_found");
    db.cleanup().await;
}

#[tokio::test]
async fn брошенный_счёт_не_мешает_оплатить_с_тем_же_промокодом() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let promo = create(&ctx, &percent("AGAIN", 10, Some(5)), None)
        .await
        .unwrap();
    let q = priced(&[promo.adjustment().unwrap()]);
    let buyer = client(&db.store).await;
    let abandoned = order(&db.store, buyer, OrderKind::New, intent(), q.clone()).await;
    let until = Utc::now() + Duration::minutes(60);
    reserve(&ctx, &abandoned, until).await.unwrap();
    assert!(
        matches!(
            check(&ctx, buyer, "again", checkout()).await,
            Err(PromoError::Rejected(PromoRejection::Reserved))
        ),
        "пока первый счёт можно оплатить, второй по тому же промокоду не выставляется"
    );
    let retry = order(&db.store, buyer, OrderKind::New, intent(), q.clone()).await;
    assert!(matches!(
        reserve(&ctx, &retry, until).await,
        Err(ModuleError::Invalid(_))
    ));
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.orders()
            .transition(abandoned.id, sn_domain::order::OrderStatus::Canceled)
            .await
            .unwrap();
    }
    assert!(
        check(&ctx, buyer, "again", checkout()).await.is_ok(),
        "отменённый счёт не держит промокод"
    );
    assert!(matches!(reserve(&ctx, &retry, until).await, Ok(Some(_))));
    let page = list(&ctx, ListFilter::All, PageRequest::default())
        .await
        .unwrap();
    assert_eq!(page.items[0].reserved, 1);
    let payment = paid_payment(
        &db.store,
        retry.id,
        buyer,
        "cryptopay",
        Amount::from(q.total),
    )
    .await;
    PromoModule
        .on_event(
            &ctx,
            &DomainEvent::PaymentPaid {
                payment_id: payment.id,
                order_id: retry.id,
                client_id: buyer,
                order_kind: OrderKind::New,
                amount: payment.amount,
                provider: payment.provider.clone(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        check(&ctx, buyer, "again", checkout()).await,
        Err(PromoError::Rejected(PromoRejection::AlreadyUsed))
    ));
    db.cleanup().await;
}

#[tokio::test]
async fn два_одновременных_нажатия_не_добавляют_дни_дважды() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut days = percent("TWICE", 1, None);
    days.value = NewPromoValue::Days { days: 3 };
    create(&ctx, &days, None).await.unwrap();
    let owner = client(&db.store).await;
    let sub = subscription(&db, owner).await;
    let (first, second) = tokio::join!(
        activate(&ctx, owner, "TWICE", sub),
        activate(&ctx, owner, "TWICE", sub)
    );
    let applied = [first.is_ok(), second.is_ok()]
        .into_iter()
        .filter(|ok| *ok)
        .count();
    assert_eq!(applied, 1);
    assert_eq!(core.days().len(), 1);
    db.cleanup().await;
}

#[tokio::test]
async fn потерянное_событие_оплаты_досводится_задачей() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let promo = create(&ctx, &percent("LOST", 10, Some(1)), None)
        .await
        .unwrap();
    let q = priced(&[promo.adjustment().unwrap()]);
    let buyer = client(&db.store).await;
    let paid = order(&db.store, buyer, OrderKind::New, intent(), q.clone()).await;
    paid_payment(
        &db.store,
        paid.id,
        buyer,
        "cryptopay",
        Amount::from(q.total),
    )
    .await;
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.orders()
            .transition(paid.id, sn_domain::order::OrderStatus::Paid)
            .await
            .unwrap();
    }
    assert_eq!(settle(&ctx, 10).await.unwrap(), 1);
    assert_eq!(settle(&ctx, 10).await.unwrap(), 0);
    let other = client(&db.store).await;
    assert!(matches!(
        check(&ctx, other, "LOST", checkout()).await,
        Err(PromoError::Rejected(PromoRejection::Exhausted))
    ));
    db.cleanup().await;
}

fn customer(client_id: ClientId) -> ClientPrincipal {
    ClientPrincipal {
        client_id,
        channel: ClientChannel::Cabinet,
        full_access: true,
        session_id: Some(1),
    }
}

async fn post(
    router: &axum::Router,
    who: ClientId,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .extension(customer(who))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

async fn use_up(ctx: &ModuleContext, store: &sn_store::Store, code: &PromoCode) {
    let user = client(store).await;
    let q = priced(&[code.adjustment().unwrap()]);
    let done = order(store, user, OrderKind::New, intent(), q.clone()).await;
    let payment = paid_payment(store, done.id, user, "cryptopay", Amount::from(q.total)).await;
    PromoModule
        .on_event(
            ctx,
            &DomainEvent::PaymentPaid {
                payment_id: payment.id,
                order_id: done.id,
                client_id: user,
                order_kind: OrderKind::New,
                amount: payment.amount,
                provider: payment.provider.clone(),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn несуществующий_и_неподходящий_код_неотличимы_для_клиента() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut expired = percent("OLD10", 10, None);
    expired.valid_until = Some(Utc::now() - Duration::days(1));
    create(&ctx, &expired, None).await.unwrap();
    let mut later = percent("SOON10", 10, None);
    later.valid_from = Some(Utc::now() + Duration::days(1));
    create(&ctx, &later, None).await.unwrap();
    let off = create(&ctx, &percent("OFF10", 10, None), None)
        .await
        .unwrap();
    set_active(&ctx, off.id, false).await.unwrap();
    let full = create(&ctx, &percent("FULL10", 10, Some(1)), None)
        .await
        .unwrap();
    use_up(&ctx, &db.store, &full).await;
    let mut days_old = percent("WEEKOLD", 1, None);
    days_old.value = NewPromoValue::Days { days: 7 };
    days_old.valid_until = Some(Utc::now() - Duration::days(1));
    create(&ctx, &days_old, None).await.unwrap();
    let mut days_full = percent("WEEKFULL", 1, Some(1));
    days_full.value = NewPromoValue::Days { days: 7 };
    create(&ctx, &days_full, None).await.unwrap();
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx.clone());
    let filler = client(&db.store).await;
    let filler_sub = subscription(&db, filler).await;
    activate(&ctx, filler, "WEEKFULL", filler_sub)
        .await
        .unwrap();
    let buyer = client(&db.store).await;
    let sub = subscription(&db, buyer).await;
    let missing = post(
        &router,
        buyer,
        "/promo/check",
        serde_json::json!({"code": "NOSUCH10"}),
    )
    .await;
    assert_eq!(missing.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(missing.1["error"]["code"], "promo.not_found");
    for code in ["OLD10", "SOON10", "OFF10", "FULL10"] {
        let answer = post(
            &router,
            buyer,
            "/promo/check",
            serde_json::json!({ "code": code }),
        )
        .await;
        assert_eq!(answer, missing, "{code}");
    }
    let missing_days = post(
        &router,
        buyer,
        "/promo/activate",
        serde_json::json!({"code": "NOSUCH7", "subscription_id": sub}),
    )
    .await;
    assert_eq!(missing_days, missing);
    for code in ["WEEKOLD", "WEEKFULL", "OFF10"] {
        let answer = post(
            &router,
            buyer,
            "/promo/activate",
            serde_json::json!({ "code": code, "subscription_id": sub }),
        )
        .await;
        assert_eq!(answer, missing, "{code}");
    }
    db.cleanup().await;
}

#[tokio::test]
async fn перебор_промокодов_останавливается_после_серии_ошибок() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    create(&ctx, &percent("REAL20", 20, None), None)
        .await
        .unwrap();
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx.clone());
    let guesser = client(&db.store).await;
    let honest = client(&db.store).await;
    for n in 0..10 {
        let (status, _) = post(
            &router,
            guesser,
            "/promo/check",
            serde_json::json!({ "code": format!("GUESS{n}") }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "попытка {n}");
    }
    let (status, body) = post(
        &router,
        guesser,
        "/promo/check",
        serde_json::json!({"code": "REAL20"}),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "promo.rate_limited");
    let sub = subscription(&db, guesser).await;
    let (status, _) = post(
        &router,
        guesser,
        "/promo/activate",
        serde_json::json!({"code": "REAL20", "subscription_id": sub}),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        PromoModule
            .quote_adjustments(&ctx, &hook(guesser, "REAL20"))
            .await
            .is_err()
    );
    assert!(matches!(
        check(&ctx, guesser, "REAL20", checkout()).await,
        Err(PromoError::Rejected(PromoRejection::TooManyAttempts))
    ));
    let (status, body) = post(
        &router,
        honest,
        "/promo/check",
        serde_json::json!({"code": "REAL20"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    db.cleanup().await;
}

async fn invoice(
    store: &sn_store::Store,
    buyer: ClientId,
    q: &Quote,
    expires_at: Option<DateTime<Utc>>,
) -> (Order, sn_domain::payment::Payment) {
    let created = order(store, buyer, OrderKind::New, intent(), q.clone()).await;
    let mut conn = store.conn().await.unwrap();
    let payment = conn
        .payments()
        .create(&sn_domain::payment::NewPayment {
            order_id: created.id,
            client_id: buyer,
            provider: sn_domain::payment::ProviderId::from("cryptopay"),
            method: None,
            amount: Amount::from(q.total),
            expires_at,
        })
        .await
        .unwrap();
    (created, payment)
}

async fn created(
    ctx: &ModuleContext,
    store: &sn_store::Store,
    order: &Order,
    payment: &sn_domain::payment::Payment,
) -> Result<(), ModuleError> {
    let mut tx = store.begin().await.unwrap();
    PromoModule
        .on_order_created(ctx, &mut tx, order, payment)
        .await?;
    tx.commit().await.unwrap();
    Ok(())
}

async fn pay(
    ctx: &ModuleContext,
    store: &sn_store::Store,
    order: &Order,
    payment: &sn_domain::payment::Payment,
) -> Result<(), ModuleError> {
    let mut tx = store.begin().await.unwrap();
    let (order, _) = tx
        .orders()
        .transition(order.id, sn_domain::order::OrderStatus::Paid)
        .await
        .unwrap();
    PromoModule
        .on_order_paid(ctx, &mut tx, &order, payment)
        .await?;
    tx.commit().await.unwrap();
    Ok(())
}

#[tokio::test]
async fn резерв_создаётся_в_транзакции_заказа_и_снимается_при_отмене() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let promo = create(&ctx, &percent("TXONE", 10, Some(1)), None)
        .await
        .unwrap();
    let q = priced(&[promo.adjustment().unwrap()]);
    let first = client(&db.store).await;
    let second = client(&db.store).await;
    let (dropped, dropped_payment) = invoice(
        &db.store,
        first,
        &q,
        Some(Utc::now() + Duration::minutes(60)),
    )
    .await;
    {
        let mut tx = db.store.begin().await.unwrap();
        PromoModule
            .on_order_created(&ctx, &mut tx, &dropped, &dropped_payment)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
    }
    let page = list(&ctx, ListFilter::All, PageRequest::default())
        .await
        .unwrap();
    assert_eq!((page.items[0].used, page.items[0].reserved), (0, 0));
    let (held, held_payment) = invoice(
        &db.store,
        first,
        &q,
        Some(Utc::now() + Duration::minutes(60)),
    )
    .await;
    created(&ctx, &db.store, &held, &held_payment)
        .await
        .unwrap();
    let (other, other_payment) = invoice(
        &db.store,
        second,
        &q,
        Some(Utc::now() + Duration::minutes(60)),
    )
    .await;
    assert!(matches!(
        created(&ctx, &db.store, &other, &other_payment).await,
        Err(ModuleError::Invalid(_))
    ));
    {
        let mut tx = db.store.begin().await.unwrap();
        let (canceled, _) = tx
            .orders()
            .transition(held.id, sn_domain::order::OrderStatus::Canceled)
            .await
            .unwrap();
        PromoModule
            .on_order_canceled(&ctx, &mut tx, &canceled)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    created(&ctx, &db.store, &other, &other_payment)
        .await
        .unwrap();
    let page = list(&ctx, ListFilter::All, PageRequest::default())
        .await
        .unwrap();
    assert_eq!((page.items[0].used, page.items[0].reserved), (0, 1));
    db.cleanup().await;
}

#[tokio::test]
async fn поздняя_оплата_истёкшего_счёта_перепроверяет_лимит() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let single = create(&ctx, &percent("LATE1", 10, Some(1)), None)
        .await
        .unwrap();
    let double = create(&ctx, &percent("LATE2", 10, Some(2)), None)
        .await
        .unwrap();
    for (promo, room) in [(single, false), (double, true)] {
        let q = priced(&[promo.adjustment().unwrap()]);
        let slow = client(&db.store).await;
        let quick = client(&db.store).await;
        let (stale, stale_payment) =
            invoice(&db.store, slow, &q, Some(Utc::now() - Duration::minutes(1))).await;
        created(&ctx, &db.store, &stale, &stale_payment)
            .await
            .unwrap();
        let (fresh, fresh_payment) = invoice(
            &db.store,
            quick,
            &q,
            Some(Utc::now() + Duration::minutes(60)),
        )
        .await;
        created(&ctx, &db.store, &fresh, &fresh_payment)
            .await
            .unwrap();
        pay(&ctx, &db.store, &fresh, &fresh_payment).await.unwrap();
        let late = pay(&ctx, &db.store, &stale, &stale_payment).await;
        assert_eq!(late.is_ok(), room, "{}: {late:?}", promo.code);
    }
    db.cleanup().await;
}

#[tokio::test]
async fn скидки_на_первую_покупку_не_складываются_в_разных_счетах() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut welcome = percent("WELCOME", 30, None);
    welcome.first_purchase_only = true;
    let welcome = create(&ctx, &welcome, None).await.unwrap();
    let mut hello = percent("HELLO", 20, None);
    hello.first_purchase_only = true;
    let hello = create(&ctx, &hello, None).await.unwrap();
    let buyer = client(&db.store).await;
    let until = Some(Utc::now() + Duration::minutes(60));
    let (first, first_payment) = invoice(
        &db.store,
        buyer,
        &priced(&[welcome.adjustment().unwrap()]),
        until,
    )
    .await;
    created(&ctx, &db.store, &first, &first_payment)
        .await
        .unwrap();
    let (second, second_payment) = invoice(
        &db.store,
        buyer,
        &priced(&[hello.adjustment().unwrap()]),
        until,
    )
    .await;
    assert!(matches!(
        created(&ctx, &db.store, &second, &second_payment).await,
        Err(ModuleError::Invalid(_))
    ));
    assert!(matches!(
        check(&ctx, buyer, "HELLO", checkout()).await,
        Err(PromoError::Rejected(PromoRejection::FirstPurchaseOnly))
    ));
    let plain = order(&db.store, buyer, OrderKind::New, intent(), priced(&[])).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.orders()
            .transition(plain.id, sn_domain::order::OrderStatus::Paid)
            .await
            .unwrap();
    }
    pay(&ctx, &db.store, &first, &first_payment).await.unwrap();
    let page = list(&ctx, ListFilter::All, PageRequest::default())
        .await
        .unwrap();
    let welcome_row = page
        .items
        .iter()
        .find(|i| i.code.code == "WELCOME")
        .unwrap();
    assert_eq!((welcome_row.used, welcome_row.reserved), (1, 0));
    db.cleanup().await;
}
