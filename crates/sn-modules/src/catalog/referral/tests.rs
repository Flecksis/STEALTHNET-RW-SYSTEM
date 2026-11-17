use super::*;
use crate::catalog::testing::{TestCore, client, context, order, paid_payment, pooled, usd};
use sn_domain::ids::{PeriodId, TariffId};
use sn_domain::ledger::NewLedgerEntry;
use sn_domain::money::PayCurrency;
use sn_domain::order::{OrderKind, PurchaseIntent};
use sn_domain::pricing::{Quote, QuoteLine, QuoteLineKind};
use sn_store::Store;
use sn_store::testing::test_store;

fn quote(total: Money) -> Quote {
    Quote {
        currency: total.currency,
        lines: vec![QuoteLine {
            kind: QuoteLineKind::Period {
                tariff_id: TariffId(1),
                period_id: PeriodId(1),
                days: 30,
                months: 1,
            },
            label: "Стандарт · 30 дней".into(),
            amount: total,
            stars: None,
        }],
        subtotal: total,
        discount: Money::zero(total.currency),
        total,
        stars: None,
        days: 30,
        extra_devices: 0,
    }
}

async fn pay(
    store: &Store,
    payer: ClientId,
    provider: &str,
    amount: Amount,
) -> (PaymentId, DomainEvent) {
    let total = amount.as_money().unwrap_or(usd(0));
    let created = order(
        store,
        payer,
        OrderKind::New,
        PurchaseIntent::Subscription {
            tariff_id: TariffId(1),
            period_id: PeriodId(1),
            extra_devices: 0,
            target: None,
        },
        quote(total),
    )
    .await;
    let payment = paid_payment(store, created.id, payer, provider, amount).await;
    let event = DomainEvent::PaymentPaid {
        payment_id: payment.id,
        order_id: created.id,
        client_id: payer,
        order_kind: OrderKind::New,
        amount,
        provider: payment.provider.clone(),
    };
    (payment.id, event)
}

async fn referral_balance(store: &Store, client_id: ClientId) -> Money {
    let mut conn = store.conn().await.unwrap();
    conn.ledger()
        .balance(client_id, Wallet::Referral, Currency::Usd)
        .await
        .unwrap()
}

async fn invite(ctx: &ModuleContext, referrer: ClientId, invited: ClientId) {
    let code = {
        let mut conn = ctx.store.conn().await.unwrap();
        ensure_code(&mut conn, referrer).await.unwrap()
    };
    ReferralModule
        .on_event(
            ctx,
            &DomainEvent::ClientRegistered {
                client_id: invited,
                source: sn_domain::client::RegistrationSource::Bot,
                start_param: Some(format!("s_tiktok__ref_{}", code.to_lowercase())),
            },
        )
        .await
        .unwrap();
}

#[test]
fn номер_карты_проверяется_по_луну() {
    assert!(luhn("4111111111111111"));
    assert!(!luhn("4111111111111112"));
    assert_eq!(
        PayoutMethod::Card
            .normalize_details("4111 1111-1111 1111")
            .as_deref(),
        Some("4111111111111111")
    );
    assert_eq!(
        PayoutMethod::Card.normalize_details("4111 1111 1111 1112"),
        None
    );
    assert!(
        PayoutMethod::Sbp
            .normalize_details("+7 900 123-45-67, Т-Банк")
            .is_some()
    );
    assert_eq!(PayoutMethod::Sbp.normalize_details("Т-Банк"), None);
    assert!(
        PayoutMethod::UsdtTrc20
            .normalize_details("TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC")
            .is_some()
    );
    assert_eq!(PayoutMethod::UsdtTrc20.normalize_details("0x1234"), None);
}

#[test]
fn настройки_рефералов_по_умолчанию_и_с_проверкой() {
    let defaults = ReferralSettings::parse(&ModuleConfig::default(), Currency::Usd).unwrap();
    assert_eq!(defaults.levels, [30, 10, 10]);
    assert_eq!(defaults.withdraw_min, None);
    assert!(defaults.withdrawals);
    let custom = ModuleConfig::from_value(&serde_json::json!({
        "level1_percent": "20", "level2_percent": 0, "level3_percent": "", "withdraw_min": "10.50",
        "withdrawals": false
    }));
    let parsed = ReferralSettings::parse(&custom, Currency::Usd).unwrap();
    assert_eq!(parsed.levels, [20, 0, 10]);
    assert_eq!(parsed.withdraw_min, Some(usd(1050)));
    assert!(!parsed.withdrawals);
    let greedy = ModuleConfig::from_value(&serde_json::json!({"level1_percent": 90}));
    assert!(ReferralModule.validate_config(&greedy).is_err());
    let broken = ModuleConfig::from_value(&serde_json::json!({"level2_percent": "много"}));
    assert!(ReferralSettings::parse(&broken, Currency::Usd).is_err());
}

#[test]
fn код_и_ссылка_приглашения() {
    let code = generate_code();
    assert_eq!(code.len(), 8);
    assert!(code.bytes().all(|b| CODE_ALPHABET.contains(&b)));
    assert_eq!(
        invite_link("@stealth_bot", "AB12CD34"),
        "https://t.me/stealth_bot?start=ref_AB12CD34"
    );
}

#[test]
fn начисление_только_с_внешних_оплат_в_валюте() {
    assert!(earns_referral(
        &ProviderId::from("cryptopay"),
        Amount::new(699, PayCurrency::Usd)
    ));
    assert!(!earns_referral(
        &ProviderId::from(providers::BALANCE),
        Amount::new(699, PayCurrency::Usd)
    ));
    assert!(!earns_referral(
        &ProviderId::from(providers::STARS),
        Amount::new(350, PayCurrency::Xtr)
    ));
    assert!(!earns_referral(
        &ProviderId::from(providers::MANUAL),
        Amount::new(699, PayCurrency::Usd)
    ));
}

#[test]
fn сторно_пропорционально_возврату() {
    assert_eq!(reversal_for(210, 699, 699, Currency::Usd), 210);
    assert_eq!(reversal_for(210, 699, 300, Currency::Usd), 90);
    assert_eq!(reversal_for(210, 699, 0, Currency::Usd), 0);
    assert_eq!(reversal_for(15_000, 49_900, 10_000, Currency::Rub), 3_000);
    assert_eq!(reversal_for(15_000, 49_900, 99_999, Currency::Rub), 15_000);
}

#[tokio::test]
async fn приглашение_по_ссылке_и_три_уровня_начислений() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let grand = client(&db.store).await;
    let parent = client(&db.store).await;
    let child = client(&db.store).await;
    let buyer = client(&db.store).await;
    invite(&ctx, grand, parent).await;
    invite(&ctx, parent, child).await;
    invite(&ctx, child, buyer).await;
    invite(&ctx, buyer, buyer).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        assert_eq!(
            store::referrer_of(&mut conn, buyer).await.unwrap(),
            Some(child)
        );
        assert_eq!(store::invited_count(&mut conn, child).await.unwrap(), 1);
    }
    let (payment, event) = pay(
        &db.store,
        buyer,
        "cryptopay",
        Amount::new(1000, PayCurrency::Usd),
    )
    .await;
    ReferralModule.on_event(&ctx, &event).await.unwrap();
    ReferralModule.on_event(&ctx, &event).await.unwrap();
    assert_eq!(referral_balance(&db.store, child).await, usd(300));
    assert_eq!(referral_balance(&db.store, parent).await, usd(100));
    assert_eq!(referral_balance(&db.store, grand).await, usd(100));
    assert_eq!(referral_balance(&db.store, buyer).await, usd(0));
    let mut conn = db.store.conn().await.unwrap();
    let main = conn
        .ledger()
        .balance(child, Wallet::Main, Currency::Usd)
        .await
        .unwrap();
    assert_eq!(main, usd(0));
    assert_eq!(
        store::credits_for_payment(&mut conn, payment)
            .await
            .unwrap()
            .len(),
        3
    );
    drop(conn);
    assert_eq!(
        core.notifications()
            .iter()
            .filter(|(_, n)| n.template == TEMPLATE_CREDIT)
            .count(),
        3
    );
    let (_, balance_paid) = pay(
        &db.store,
        buyer,
        providers::BALANCE,
        Amount::new(500, PayCurrency::Usd),
    )
    .await;
    ReferralModule.on_event(&ctx, &balance_paid).await.unwrap();
    let (_, stars_paid) = pay(
        &db.store,
        buyer,
        providers::STARS,
        Amount::new(350, PayCurrency::Xtr),
    )
    .await;
    ReferralModule.on_event(&ctx, &stars_paid).await.unwrap();
    assert_eq!(referral_balance(&db.store, child).await, usd(300));
    let overview = overview(&ctx, child).await.unwrap();
    assert_eq!(overview.invited, 1);
    assert_eq!(overview.earned, usd(300));
    assert_eq!(overview.balance, usd(300));
    db.cleanup().await;
}

#[tokio::test]
async fn возврат_оплаты_сторнирует_начисление_в_пределах_остатка() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let referrer = client(&db.store).await;
    let buyer = client(&db.store).await;
    invite(&ctx, referrer, buyer).await;
    let (payment, event) = pay(
        &db.store,
        buyer,
        "cryptopay",
        Amount::new(2000, PayCurrency::Usd),
    )
    .await;
    ReferralModule.on_event(&ctx, &event).await.unwrap();
    assert_eq!(referral_balance(&db.store, referrer).await, usd(600));
    request_withdrawal(
        &ctx,
        referrer,
        &WithdrawalRequest {
            amount: "5.00".into(),
            method: PayoutMethod::Card,
            details: "4111 1111 1111 1111".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(referral_balance(&db.store, referrer).await, usd(100));
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.payments()
            .transition(payment, PaymentStatus::Refunded, None, None)
            .await
            .unwrap();
    }
    let refunded = DomainEvent::PaymentRefunded {
        payment_id: payment,
        order_id: sn_domain::ids::OrderId(0),
        client_id: buyer,
        amount: Amount::new(2000, PayCurrency::Usd),
    };
    ReferralModule.on_event(&ctx, &refunded).await.unwrap();
    ReferralModule.on_event(&ctx, &refunded).await.unwrap();
    assert_eq!(referral_balance(&db.store, referrer).await, usd(0));
    {
        let mut conn = db.store.conn().await.unwrap();
        let credit = &store::credits_for_payment(&mut conn, payment)
            .await
            .unwrap()[0];
        assert_eq!(
            (credit.reversal_target_minor, credit.reversed_minor),
            (600, 100)
        );
    }
    {
        let mut tx = db.store.begin().await.unwrap();
        tx.ledger()
            .append(&NewLedgerEntry {
                client_id: referrer,
                wallet: Wallet::Referral,
                amount: usd(700),
                kind: LedgerKind::Contest,
                order_id: None,
                payment_id: None,
                admin_id: None,
                comment: Some("приз".into()),
                idempotency_key: Some("test:contest".into()),
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let report = settle(&ctx, 50).await.unwrap();
    assert_eq!(report.credits, 1);
    assert_eq!(referral_balance(&db.store, referrer).await, usd(200));
    assert_eq!(settle(&ctx, 50).await.unwrap(), SettleReport::default());
    db.cleanup().await;
}

#[tokio::test]
async fn вывод_только_из_реферального_кошелька_с_возвратом_при_отказе() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let referrer = client(&db.store).await;
    let buyer = client(&db.store).await;
    {
        let mut tx = db.store.begin().await.unwrap();
        tx.ledger()
            .append(&NewLedgerEntry {
                client_id: referrer,
                wallet: Wallet::Main,
                amount: usd(10_000),
                kind: LedgerKind::TopUp,
                order_id: None,
                payment_id: None,
                admin_id: None,
                comment: None,
                idempotency_key: Some("test:topup".into()),
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let ask = |amount: &str| WithdrawalRequest {
        amount: amount.into(),
        method: PayoutMethod::UsdtTrc20,
        details: "TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC".into(),
    };
    assert!(matches!(
        request_withdrawal(&ctx, referrer, &ask("1.00")).await,
        Err(ReferralError::Rejected(ReferralRejection::NotEnough { .. }))
    ));
    invite(&ctx, referrer, buyer).await;
    let (_, event) = pay(
        &db.store,
        buyer,
        "heleket",
        Amount::new(5000, PayCurrency::Usd),
    )
    .await;
    ReferralModule.on_event(&ctx, &event).await.unwrap();
    assert_eq!(referral_balance(&db.store, referrer).await, usd(1500));
    assert!(matches!(
        request_withdrawal(&ctx, referrer, &ask("abc")).await,
        Err(ReferralError::Rejected(ReferralRejection::BadAmount))
    ));
    assert!(matches!(
        request_withdrawal(
            &ctx,
            referrer,
            &WithdrawalRequest {
                details: "не адрес".into(),
                ..ask("1.00")
            }
        )
        .await,
        Err(ReferralError::Rejected(ReferralRejection::BadDetails))
    ));
    let first = request_withdrawal(&ctx, referrer, &ask("10.00"))
        .await
        .unwrap();
    assert_eq!(first.status, WithdrawalStatus::Pending);
    assert_eq!(referral_balance(&db.store, referrer).await, usd(500));
    assert!(matches!(
        decide(
            &ctx,
            first.id,
            &Decision::Rejected {
                comment: " ".into()
            },
            None
        )
        .await,
        Err(ReferralError::Rejected(ReferralRejection::NeedComment))
    ));
    let rejected = decide(
        &ctx,
        first.id,
        &Decision::Rejected {
            comment: "Неверный адрес".into(),
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(rejected.status, WithdrawalStatus::Rejected);
    assert_eq!(referral_balance(&db.store, referrer).await, usd(1500));
    assert!(matches!(
        decide(&ctx, first.id, &Decision::Paid { comment: None }, None).await,
        Err(ReferralError::Rejected(ReferralRejection::AlreadyDecided))
    ));
    let second = request_withdrawal(&ctx, referrer, &ask("15.00"))
        .await
        .unwrap();
    let paid = decide(&ctx, second.id, &Decision::Paid { comment: None }, None)
        .await
        .unwrap();
    assert_eq!(paid.status, WithdrawalStatus::Paid);
    assert_eq!(referral_balance(&db.store, referrer).await, usd(0));
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(
        conn.ledger()
            .balance(referrer, Wallet::Main, Currency::Usd)
            .await
            .unwrap(),
        usd(10_000)
    );
    drop(conn);
    let templates: Vec<String> = core
        .notifications()
        .into_iter()
        .map(|(_, n)| n.template)
        .collect();
    assert!(templates.contains(&TEMPLATE_WITHDRAWAL_REJECTED.to_string()));
    assert!(templates.contains(&TEMPLATE_WITHDRAWAL_PAID.to_string()));
    db.cleanup().await;
}

#[tokio::test]
async fn клиент_отменяет_свою_заявку_и_деньги_возвращаются() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let referrer = client(&db.store).await;
    let stranger = client(&db.store).await;
    let buyer = client(&db.store).await;
    invite(&ctx, referrer, buyer).await;
    let (_, event) = pay(
        &db.store,
        buyer,
        "cryptopay",
        Amount::new(1000, PayCurrency::Usd),
    )
    .await;
    ReferralModule.on_event(&ctx, &event).await.unwrap();
    let request = request_withdrawal(
        &ctx,
        referrer,
        &WithdrawalRequest {
            amount: "3".into(),
            method: PayoutMethod::Sbp,
            details: "+7 900 123-45-67, Т-Банк".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(referral_balance(&db.store, referrer).await, usd(0));
    assert!(matches!(
        cancel_withdrawal(&ctx, stranger, request.id).await,
        Err(ReferralError::Module(ModuleError::NotFound(_)))
    ));
    let canceled = cancel_withdrawal(&ctx, referrer, request.id).await.unwrap();
    assert_eq!(canceled.status, WithdrawalStatus::Canceled);
    assert_eq!(referral_balance(&db.store, referrer).await, usd(300));
    assert_eq!(settle(&ctx, 10).await.unwrap(), SettleReport::default());
    db.cleanup().await;
}

#[tokio::test]
async fn пропущенная_оплата_начисляется_задачей_один_раз() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let referrer = client(&db.store).await;
    let buyer = client(&db.store).await;
    invite(&ctx, referrer, buyer).await;
    let (_, event) = pay(
        &db.store,
        buyer,
        "platega",
        Amount::new(1000, PayCurrency::Usd),
    )
    .await;
    let report = settle(&ctx, 50).await.unwrap();
    assert_eq!(report.payments, 1);
    assert_eq!(referral_balance(&db.store, referrer).await, usd(300));
    ReferralModule.on_event(&ctx, &event).await.unwrap();
    assert_eq!(settle(&ctx, 50).await.unwrap().payments, 0);
    assert_eq!(referral_balance(&db.store, referrer).await, usd(300));
    db.cleanup().await;
}

async fn refund(store: &Store, payment: PaymentId, buyer: ClientId, amount: i64) -> DomainEvent {
    let mut conn = store.conn().await.unwrap();
    conn.payments()
        .transition(payment, PaymentStatus::Refunded, None, None)
        .await
        .unwrap();
    DomainEvent::PaymentRefunded {
        payment_id: payment,
        order_id: sn_domain::ids::OrderId(0),
        client_id: buyer,
        amount: Amount::new(amount, PayCurrency::Usd),
    }
}

async fn move_referral(store: &Store, client_id: ClientId, minor: i64, key: &str) {
    let mut tx = store.begin().await.unwrap();
    tx.ledger()
        .append(&NewLedgerEntry {
            client_id,
            wallet: Wallet::Referral,
            amount: usd(minor),
            kind: LedgerKind::Contest,
            order_id: None,
            payment_id: None,
            admin_id: None,
            comment: None,
            idempotency_key: Some(key.into()),
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn the_credit(store: &Store, payment: PaymentId) -> store::Credit {
    let mut conn = store.conn().await.unwrap();
    store::credits_for_payment(&mut conn, payment)
        .await
        .unwrap()
        .remove(0)
}

async fn settle_at_once(ctx: &ModuleContext, id: i64, times: usize) -> Vec<bool> {
    let mut handles = Vec::new();
    for _ in 0..times {
        let ctx = ctx.clone();
        handles.push(tokio::spawn(async move { settle_credit(&ctx, id).await }));
    }
    let mut done = Vec::new();
    for handle in handles {
        done.push(handle.await.unwrap().unwrap());
    }
    done
}

#[tokio::test]
async fn начисление_и_сторно_обходятся_одним_соединением() {
    let Some(db) = test_store().await else {
        return;
    };
    let setup_core = TestCore::new(&db.store);
    let setup = context(&db.store, &setup_core, Currency::Usd);
    let referrer = client(&db.store).await;
    let buyer = client(&db.store).await;
    invite(&setup, referrer, buyer).await;
    let (payment, paid) = pay(
        &db.store,
        buyer,
        "cryptopay",
        Amount::new(1000, PayCurrency::Usd),
    )
    .await;
    let single = pooled(&db, 1, Duration::from_secs(2)).await;
    let core = TestCore::new(&single);
    let ctx = context(&single, &core, Currency::Usd);
    ReferralModule.on_event(&ctx, &paid).await.unwrap();
    assert_eq!(referral_balance(&db.store, referrer).await, usd(300));
    let refunded = refund(&db.store, payment, buyer, 1000).await;
    ReferralModule.on_event(&ctx, &refunded).await.unwrap();
    assert_eq!(referral_balance(&db.store, referrer).await, usd(0));
    assert_eq!(settle(&ctx, 50).await.unwrap(), SettleReport::default());
    let credit = the_credit(&db.store, payment).await;
    assert_eq!(
        (credit.reversal_target_minor, credit.reversed_minor),
        (300, 300)
    );
    single.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn одновременная_обработка_начисления_зачисляет_и_сторнирует_один_раз() {
    let Some(db) = test_store().await else {
        return;
    };
    let wide = pooled(&db, 8, Duration::from_secs(8)).await;
    let core = TestCore::new(&wide);
    let ctx = context(&wide, &core, Currency::Usd);
    let referrer = client(&db.store).await;
    let buyer = client(&db.store).await;
    invite(&ctx, referrer, buyer).await;
    let (payment, _) = pay(
        &db.store,
        buyer,
        "cryptopay",
        Amount::new(1000, PayCurrency::Usd),
    )
    .await;
    {
        let mut conn = db.store.conn().await.unwrap();
        assert!(store::mark_processed(&mut conn, payment).await.unwrap());
        assert!(
            store::insert_credit(
                &mut conn,
                &store::NewCredit {
                    payment_id: payment,
                    level: 1,
                    referrer_id: referrer,
                    referral_id: buyer,
                    base: usd(1000),
                    percent: 30,
                    amount: usd(300),
                },
            )
            .await
            .unwrap()
        );
    }
    let id = the_credit(&db.store, payment).await.id;
    let credited = settle_at_once(&ctx, id, 8).await;
    assert!(credited.iter().all(|done| *done), "{credited:?}");
    assert_eq!(referral_balance(&db.store, referrer).await, usd(300));
    assert!(the_credit(&db.store, payment).await.credited_at.is_some());
    assert_eq!(
        core.notifications()
            .iter()
            .filter(|(_, n)| n.template == TEMPLATE_CREDIT)
            .count(),
        1
    );
    move_referral(&db.store, referrer, -200, "test:spent").await;
    {
        let mut conn = db.store.conn().await.unwrap();
        store::set_reversal_target(&mut conn, id, 300)
            .await
            .unwrap();
    }
    let partial = settle_at_once(&ctx, id, 8).await;
    assert!(partial.iter().all(|done| !*done), "{partial:?}");
    assert_eq!(referral_balance(&db.store, referrer).await, usd(0));
    assert_eq!(the_credit(&db.store, payment).await.reversed_minor, 100);
    move_referral(&db.store, referrer, 500, "test:prize").await;
    let full = settle_at_once(&ctx, id, 8).await;
    assert!(full.iter().all(|done| *done), "{full:?}");
    assert_eq!(referral_balance(&db.store, referrer).await, usd(300));
    assert_eq!(the_credit(&db.store, payment).await.reversed_minor, 300);
    assert_eq!(settle_at_once(&ctx, id, 4).await, vec![true; 4]);
    assert_eq!(referral_balance(&db.store, referrer).await, usd(300));
    wide.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn сторно_после_сбоя_берёт_сумму_из_журнала_баланса() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let referrer = client(&db.store).await;
    let buyer = client(&db.store).await;
    invite(&ctx, referrer, buyer).await;
    let (payment, paid) = pay(
        &db.store,
        buyer,
        "cryptopay",
        Amount::new(1000, PayCurrency::Usd),
    )
    .await;
    ReferralModule.on_event(&ctx, &paid).await.unwrap();
    let id = the_credit(&db.store, payment).await.id;
    {
        let mut conn = db.store.conn().await.unwrap();
        store::set_reversal_target(&mut conn, id, 300)
            .await
            .unwrap();
    }
    ctx.core
        .change_balance(BalanceChange {
            client_id: referrer,
            wallet: Wallet::Referral,
            amount: usd(-120),
            kind: LedgerKind::ReferralReversal,
            comment: None,
            idempotency_key: format!("referral_reversal:{payment}:1:0"),
            order_id: None,
            payment_id: Some(payment),
        })
        .await
        .unwrap();
    assert_eq!(referral_balance(&db.store, referrer).await, usd(180));
    assert!(!settle_credit(&ctx, id).await.unwrap());
    assert_eq!(
        the_credit(&db.store, payment).await.reversed_minor,
        120,
        "учтено ровно то, что уже списано под этим ключом"
    );
    assert_eq!(referral_balance(&db.store, referrer).await, usd(180));
    assert!(settle_credit(&ctx, id).await.unwrap());
    assert_eq!(the_credit(&db.store, payment).await.reversed_minor, 300);
    assert_eq!(referral_balance(&db.store, referrer).await, usd(0));
    db.cleanup().await;
}
