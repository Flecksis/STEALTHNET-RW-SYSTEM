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
fn реквизиты_проверяются_по_выбранной_проверке() {
    assert!(luhn("4111111111111111"));
    assert!(!luhn("4111111111111112"));
    assert_eq!(
        DetailsCheck::Card
            .normalize("4111 1111-1111 1111")
            .as_deref(),
        Some("4111111111111111")
    );
    assert_eq!(DetailsCheck::Card.normalize("4111 1111 1111 1112"), None);
    assert_eq!(DetailsCheck::Card.normalize("4111 1111"), None);
    assert_eq!(
        DetailsCheck::Phone
            .normalize("  +7 900  123-45-67,\nТ-Банк ")
            .as_deref(),
        Some("+7 900 123-45-67, Т-Банк")
    );
    assert_eq!(DetailsCheck::Phone.normalize("Т-Банк"), None);
    assert_eq!(DetailsCheck::Phone.normalize("+7 900 12"), None);
    assert_eq!(
        DetailsCheck::Trc20
            .normalize(" TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC ")
            .as_deref(),
        Some("TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC")
    );
    assert_eq!(DetailsCheck::Trc20.normalize("0x1234"), None);
    assert_eq!(
        DetailsCheck::Trc20.normalize("TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDe0"),
        None
    );
    assert_eq!(
        DetailsCheck::Text
            .normalize("  me@example.com \n PayPal ")
            .as_deref(),
        Some("me@example.com PayPal")
    );
    assert_eq!(DetailsCheck::Text.normalize("   "), None);
    assert_eq!(
        DetailsCheck::Text.normalize(&"я".repeat(DETAILS_MAX)),
        Some("я".repeat(DETAILS_MAX))
    );
    assert_eq!(
        DetailsCheck::Text.normalize(&"я".repeat(DETAILS_MAX + 1)),
        None
    );
    for check in DetailsCheck::ALL {
        assert_eq!(DetailsCheck::parse(check.as_str()), Some(check));
    }
    assert_eq!(DetailsCheck::parse("iban"), None);
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
            method: "card".into(),
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
        method: "usdt_trc20".into(),
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
            method: "sbp".into(),
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

fn admin_as(
    admin_id: AdminId,
    role: sn_domain::access::AdminRole,
) -> sn_domain::access::AdminPrincipal {
    sn_domain::access::AdminPrincipal {
        admin_id,
        role,
        permissions: Vec::new(),
        session_id: 1,
        api_key: None,
    }
}

async fn owner(store: &Store) -> AdminId {
    let mut conn = store.conn().await.unwrap();
    conn.admins()
        .create(&sn_store::NewAdmin {
            email: "owner@example.com".into(),
            password_hash: "x".into(),
            role: sn_domain::access::AdminRole::Owner,
            must_change_password: false,
        })
        .await
        .unwrap()
        .id
}

async fn named(store: &Store, telegram: &str, first: &str, last: &str) -> ClientId {
    let id = client(store).await;
    let mut conn = store.conn().await.unwrap();
    conn.clients()
        .add_identity(
            id,
            &sn_domain::client::NewIdentity {
                kind: sn_domain::client::IdentityKind::Telegram,
                value: telegram.into(),
                verified: true,
                meta: serde_json::json!({"first_name": first, "last_name": last, "username": "anna"}),
            },
        )
        .await
        .unwrap();
    id
}

async fn get_json(
    router: &axum::Router,
    path: &str,
    principal: &sn_domain::access::AdminPrincipal,
) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let response = router
        .clone()
        .oneshot(
            axum::http::Request::get(path)
                .extension(principal.clone())
                .body(axum::body::Body::empty())
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

#[tokio::test]
async fn админка_видит_очередь_историю_сводку_и_рефереров() {
    use axum::http::StatusCode;
    use sn_domain::access::AdminRole;
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let registry =
        std::sync::Arc::new(crate::registry::ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.admin_router(ctx.clone());
    let owner_id = owner(&db.store).await;
    let boss = admin_as(owner_id, AdminRole::Owner);
    let anna = named(&db.store, "1001", "Анна", "Ким").await;
    let boris = client(&db.store).await;
    let partner = client(&db.store).await;
    let first_buyer = client(&db.store).await;
    let quiet_buyer = client(&db.store).await;
    let second_buyer = client(&db.store).await;
    invite(&ctx, anna, first_buyer).await;
    invite(&ctx, anna, quiet_buyer).await;
    invite(&ctx, boris, second_buyer).await;
    for (buyer, minor) in [(first_buyer, 5000), (second_buyer, 1000)] {
        let (_, event) = pay(
            &db.store,
            buyer,
            "heleket",
            Amount::new(minor, PayCurrency::Usd),
        )
        .await;
        ReferralModule.on_event(&ctx, &event).await.unwrap();
    }
    {
        let mut conn = db.store.conn().await.unwrap();
        ensure_code(&mut conn, partner).await.unwrap();
        store::set_personal_percent(&mut conn, partner, Some(50))
            .await
            .unwrap();
    }
    let ask = |amount: &str| WithdrawalRequest {
        amount: amount.into(),
        method: "usdt_trc20".into(),
        details: "TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC".into(),
    };
    let paid = request_withdrawal(&ctx, anna, &ask("5.00")).await.unwrap();
    let waiting = request_withdrawal(&ctx, anna, &ask("4.00")).await.unwrap();
    let later = request_withdrawal(&ctx, boris, &ask("2.00")).await.unwrap();
    decide(
        &ctx,
        paid.id,
        &Decision::Paid {
            comment: Some("Перевод 12:40".into()),
        },
        Some(owner_id),
    )
    .await
    .unwrap();

    let (status, queue) = get_json(&router, "/referral/withdrawals?status=pending", &boss).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<i64> = queue["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["id"].as_i64().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec![waiting.id, later.id],
        "очередь — от старых к новым"
    );
    assert_eq!(queue["items"][0]["client"]["name"], "Анна Ким");
    assert_eq!(queue["items"][0]["client"]["telegram_username"], "anna");
    assert_eq!(queue["items"][0]["amount"]["minor"], 400);
    assert_eq!(
        queue["items"][0]["details"],
        "TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC"
    );
    assert_eq!(queue["items"][0]["method"], "usdt_trc20");
    assert_eq!(queue["items"][0]["method_title"], "USDT (TRC20)");
    assert_eq!(queue["items"][1]["client"]["name"], serde_json::Value::Null);

    let (_, first_page) = get_json(
        &router,
        "/referral/withdrawals?status=pending&limit=1",
        &boss,
    )
    .await;
    assert_eq!(first_page["items"][0]["id"], waiting.id);
    let (_, second_page) = get_json(
        &router,
        &format!(
            "/referral/withdrawals?status=pending&limit=1&after={}",
            first_page["next"]
        ),
        &boss,
    )
    .await;
    assert_eq!(second_page["items"][0]["id"], later.id);
    assert_eq!(second_page["next"], serde_json::Value::Null);

    let (_, history) = get_json(&router, "/referral/withdrawals?status=decided", &boss).await;
    assert_eq!(history["items"].as_array().unwrap().len(), 1);
    assert_eq!(history["items"][0]["id"], paid.id);
    assert_eq!(history["items"][0]["status"], "paid");
    assert_eq!(history["items"][0]["comment"], "Перевод 12:40");
    assert_eq!(history["items"][0]["decided_by"], "owner@example.com");
    let (bad, _) = get_json(&router, "/referral/withdrawals?status=soon", &boss).await;
    assert_eq!(bad, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, summary) = get_json(&router, "/referral/summary", &boss).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["levels"], serde_json::json!([30, 10, 10]));
    assert_eq!(summary["withdrawals_enabled"], true);
    assert_eq!(summary["payout_methods"][2]["id"], "usdt_trc20");
    assert_eq!(summary["payout_methods"][2]["title"], "USDT (TRC20)");
    assert_eq!(summary["invited"], 3);
    assert_eq!(summary["referrers"], 2);
    assert_eq!(summary["earned"]["minor"], 1800);
    assert_eq!(summary["paid_out"]["minor"], 500);
    assert_eq!(summary["pending_count"], 2);
    assert_eq!(summary["pending"]["minor"], 600);
    assert_eq!(summary["holders"], 2);
    assert_eq!(summary["balance"]["minor"], 700);

    let (_, top) = get_json(&router, "/referral/referrers?limit=2", &boss).await;
    assert_eq!(top["items"][0]["client_id"], anna.0);
    assert_eq!(top["items"][0]["client"]["name"], "Анна Ким");
    assert_eq!(top["items"][0]["invited"], 2);
    assert_eq!(top["items"][0]["earned"]["minor"], 1500);
    assert_eq!(top["items"][0]["paid_out"]["minor"], 500);
    assert_eq!(top["items"][0]["balance"]["minor"], 600);
    assert_eq!(top["items"][1]["client_id"], boris.0);
    assert_eq!(top["items"][1]["balance"]["minor"], 100);
    assert_eq!(top["next"], 2);
    let (_, rest) = get_json(&router, "/referral/referrers?limit=2&after=2", &boss).await;
    assert_eq!(rest["items"].as_array().unwrap().len(), 1);
    assert_eq!(rest["items"][0]["client_id"], partner.0);
    assert_eq!(rest["items"][0]["personal_percent"], 50);
    assert_eq!(rest["items"][0]["invited"], 0);
    assert_eq!(rest["next"], serde_json::Value::Null);

    let helper = admin_as(owner_id, AdminRole::Support);
    for path in [
        "/referral/summary",
        "/referral/referrers",
        "/referral/withdrawals",
    ] {
        let (status, _) = get_json(&router, path, &helper).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    db.cleanup().await;
}

async fn put_percent(
    router: &axum::Router,
    client: ClientId,
    principal: &sn_domain::access::AdminPrincipal,
    percent: u8,
) -> axum::http::StatusCode {
    use tower::ServiceExt;
    router
        .clone()
        .oneshot(
            axum::http::Request::put(format!("/referral/clients/{}/percent", client.0))
                .header("content-type", "application/json")
                .extension(principal.clone())
                .body(axum::body::Body::from(
                    serde_json::json!({ "percent": percent }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn личный_процент_реферала_меняется_только_с_правом_денег() {
    use axum::http::StatusCode;
    use sn_domain::access::{AdminRole, Permission};
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let registry =
        std::sync::Arc::new(crate::registry::ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.admin_router(ctx.clone());
    let owner_id = owner(&db.store).await;
    let partner = client(&db.store).await;
    let marketing = sn_domain::access::AdminPrincipal {
        permissions: vec![Permission::Marketing],
        api_key: Some(1),
        ..admin_as(owner_id, AdminRole::Owner)
    };
    assert_eq!(
        put_percent(&router, partner, &marketing, 100).await,
        StatusCode::FORBIDDEN
    );
    let percent = {
        let mut conn = db.store.conn().await.unwrap();
        store::personal_percent(&mut conn, partner).await.unwrap()
    };
    assert_eq!(percent, None);
    let money = sn_domain::access::AdminPrincipal {
        permissions: vec![Permission::Money],
        ..marketing.clone()
    };
    assert_eq!(
        put_percent(&router, partner, &money, 40).await,
        StatusCode::OK
    );
    let percent = {
        let mut conn = db.store.conn().await.unwrap();
        store::personal_percent(&mut conn, partner).await.unwrap()
    };
    assert_eq!(percent, Some(40));
    db.cleanup().await;
}

#[test]
fn без_настроек_три_встроенных_способа_вывода() {
    let defaults = ReferralSettings::parse(&ModuleConfig::default(), Currency::Rub).unwrap();
    let ids: Vec<&str> = defaults
        .payout_methods
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(ids, ["sbp", "card", "usdt_trc20"]);
    assert!(
        defaults
            .payout_methods
            .iter()
            .all(|m| m.enabled && m.min.is_none())
    );
    assert_eq!(
        defaults
            .payout_methods
            .iter()
            .map(|m| m.check)
            .collect::<Vec<_>>(),
        [DetailsCheck::Phone, DetailsCheck::Card, DetailsCheck::Trc20]
    );
    let field = ReferralModule
        .settings_schema()
        .into_iter()
        .find(|f| f.key == PAYOUT_METHODS_KEY)
        .unwrap();
    assert_eq!(field.kind, sn_domain::settings::FieldKind::List);
    let stored: serde_json::Value = serde_json::from_str(field.default.unwrap()).unwrap();
    let from_default = ReferralSettings::parse(
        &ModuleConfig::from_value(&serde_json::json!({ "payout_methods": stored })),
        Currency::Rub,
    )
    .unwrap();
    assert_eq!(from_default.payout_methods, defaults.payout_methods);
    let with_min = ReferralSettings::parse(
        &ModuleConfig::from_value(&serde_json::json!({ "withdraw_min": "500" })),
        Currency::Rub,
    )
    .unwrap();
    let options = with_min.payout_options();
    assert_eq!(options.len(), 3);
    assert!(
        options
            .iter()
            .all(|o| o.min == Some(Money::new(50_000, Currency::Rub)))
    );
    assert_eq!(options[0].title, "СБП");
    assert_eq!(
        options[0].hint,
        "Номер телефона и банк, например: +7 900 123-45-67, Т-Банк"
    );
    assert_eq!(payout_title("card", None), "Банковская карта");
    assert_eq!(payout_title("card", Some("  ")), "Банковская карта");
    assert_eq!(payout_title("card", Some(" Карта МИР ")), "Карта МИР");
    assert_eq!(payout_title("bank_old", None), "bank_old");
}

fn payout_item(patch: serde_json::Value) -> serde_json::Value {
    let mut item =
        serde_json::json!({ "id": "paypal", "title": "PayPal", "hint": "", "check": "text" });
    for (key, value) in patch.as_object().unwrap() {
        item[key.as_str()] = value.clone();
    }
    item
}

#[test]
fn способы_вывода_из_настроек_и_ошибки_при_сохранении() {
    let custom = ModuleConfig::from_value(&serde_json::json!({
        "withdraw_min": "2",
        "payout_methods": [
            { "id": "paypal", "title": " PayPal ", "hint": " Почта PayPal ", "check": "text", "min": "5.5", "enabled": true },
            { "id": "card", "title": "Карта", "hint": "", "check": "card", "min": null, "enabled": false },
            { "id": "sbp_2", "title": "СБП", "check": "phone", "min": "" },
            { "id": "usdt_trc20", "title": "USDT", "check": "trc20", "min": 10 }
        ]
    }));
    let parsed = ReferralSettings::parse(&custom, Currency::Usd).unwrap();
    assert_eq!(
        parsed.payout_methods[0],
        PayoutMethod {
            id: "paypal".into(),
            title: "PayPal".into(),
            hint: "Почта PayPal".into(),
            check: DetailsCheck::Text,
            min: Some(usd(550)),
            enabled: true,
        }
    );
    assert!(!parsed.payout_methods[1].enabled);
    assert_eq!(parsed.payout_methods[2].min, None);
    assert!(parsed.payout_methods[2].enabled);
    assert_eq!(parsed.payout_methods[3].min, Some(usd(1000)));
    assert_eq!(parsed.payout_methods[3].hint, "");
    let options = parsed.payout_options();
    assert_eq!(
        options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
        ["paypal", "sbp_2", "usdt_trc20"]
    );
    assert_eq!(
        options.iter().map(|o| o.min).collect::<Vec<_>>(),
        [Some(usd(550)), Some(usd(200)), Some(usd(1000))]
    );
    assert!(parsed.payout_method("card").is_none());
    assert!(parsed.payout_method("sbp").is_none());
    assert_eq!(
        parsed.payout_method("sbp_2").map(|m| parsed.minimum_for(m)),
        Some(Some(usd(200)))
    );

    let eleven: Vec<serde_json::Value> = (0..11)
        .map(|i| payout_item(serde_json::json!({ "id": format!("m{i}") })))
        .collect();
    for (methods, needle) in [
        (serde_json::json!("sbp,card"), "ожидается список"),
        (serde_json::json!([42]), "указан неверно"),
        (
            serde_json::json!([payout_item(serde_json::json!({ "id": "PayPal" }))]),
            "код",
        ),
        (
            serde_json::json!([payout_item(serde_json::json!({ "id": "" }))]),
            "код",
        ),
        (
            serde_json::json!([payout_item(serde_json::json!({ "id": "a".repeat(21) }))]),
            "код",
        ),
        (
            serde_json::json!([payout_item(serde_json::json!({ "id": "пейпал" }))]),
            "код",
        ),
        (
            serde_json::json!([payout_item(serde_json::json!({ "title": " " }))]),
            "название",
        ),
        (
            serde_json::json!([payout_item(serde_json::json!({ "title": "я".repeat(41) }))]),
            "название",
        ),
        (
            serde_json::json!([payout_item(serde_json::json!({ "hint": "я".repeat(121) }))]),
            "подсказка",
        ),
        (
            serde_json::json!([payout_item(serde_json::json!({ "check": "iban" }))]),
            "проверка реквизитов",
        ),
        (
            serde_json::json!([payout_item(serde_json::json!({ "check": null }))]),
            "проверка реквизитов",
        ),
        (
            serde_json::json!([payout_item(serde_json::json!({ "min": "пять" }))]),
            "минимум",
        ),
        (
            serde_json::json!([payout_item(serde_json::json!({ "min": "-5" }))]),
            "минимум",
        ),
        (
            serde_json::json!([payout_item(serde_json::json!({ "enabled": "да" }))]),
            "включён",
        ),
        (
            serde_json::json!([
                payout_item(serde_json::json!({})),
                payout_item(serde_json::json!({ "title": "Ещё PayPal" }))
            ]),
            "коды должны быть разными",
        ),
        (serde_json::json!(eleven), "не больше 10"),
        (
            serde_json::json!([payout_item(serde_json::json!({ "enabled": false }))]),
            "включите хотя бы один способ",
        ),
        (serde_json::json!([]), "включите хотя бы один способ"),
    ] {
        let config = ModuleConfig::from_value(&serde_json::json!({ "payout_methods": methods }));
        let err = ReferralSettings::parse(&config, Currency::Usd).unwrap_err();
        assert!(err.contains(needle), "{needle}: {err}");
        assert!(ReferralModule.validate_config(&config).is_err(), "{needle}");
    }
    for methods in [
        serde_json::json!([]),
        serde_json::json!([payout_item(serde_json::json!({ "enabled": false }))]),
    ] {
        let off = ModuleConfig::from_value(
            &serde_json::json!({ "withdrawals": false, "payout_methods": methods }),
        );
        let parsed = ReferralSettings::parse(&off, Currency::Usd).unwrap();
        assert!(parsed.payout_options().is_empty());
        assert!(ReferralModule.validate_config(&off).is_ok());
    }
    let broken = ModuleConfig::from_value(
        &serde_json::json!({ "level1_percent": 25, "payout_methods": "card" }),
    );
    let lenient = ReferralSettings::lenient(&broken, Currency::Usd);
    assert_eq!(lenient.levels, [25, 10, 10]);
    assert_eq!(lenient.payout_methods, PayoutMethod::builtin());
}

async fn configure(store: &Store, config: serde_json::Value) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "INSERT INTO module_states (module, enabled, config) VALUES ('referral', true, $1) \
         ON CONFLICT (module) DO UPDATE SET enabled = true, config = EXCLUDED.config",
    )
    .bind(config)
    .execute(conn.conn())
    .await
    .unwrap();
}

fn paypal_config() -> serde_json::Value {
    serde_json::json!({
        "withdraw_min": "2",
        "payout_methods": [
            { "id": "paypal", "title": "PayPal", "hint": "Почта PayPal", "check": "text", "min": "5", "enabled": true },
            { "id": "usdt_trc20", "title": "USDT", "hint": "Адрес TRC20", "check": "trc20", "enabled": true },
            { "id": "sbp", "title": "СБП", "hint": "", "check": "phone", "enabled": false }
        ]
    })
}

fn ask_with(method: &str, amount: &str, details: &str) -> WithdrawalRequest {
    WithdrawalRequest {
        amount: amount.into(),
        method: method.into(),
        details: details.into(),
    }
}

#[tokio::test]
async fn заявка_своим_способом_с_его_минимумом() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    configure(&db.store, paypal_config()).await;
    let referrer = client(&db.store).await;
    move_referral(&db.store, referrer, 2000, "test:prize").await;
    let wallet = "TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC";
    assert!(matches!(
        request_withdrawal(&ctx, referrer, &ask_with("paypal", "4.99", "me@example.com")).await,
        Err(ReferralError::Rejected(ReferralRejection::BelowMinimum { min })) if min == usd(500)
    ));
    assert!(matches!(
        request_withdrawal(&ctx, referrer, &ask_with("usdt_trc20", "1.99", wallet)).await,
        Err(ReferralError::Rejected(ReferralRejection::BelowMinimum { min })) if min == usd(200)
    ));
    for method in ["sbp", "card", "PAYPAL", ""] {
        assert!(
            matches!(
                request_withdrawal(&ctx, referrer, &ask_with(method, "6", "+7 900 123-45-67"))
                    .await,
                Err(ReferralError::Rejected(
                    ReferralRejection::MethodUnavailable
                ))
            ),
            "{method}"
        );
    }
    assert!(matches!(
        request_withdrawal(
            &ctx,
            referrer,
            &ask_with("usdt_trc20", "3", "me@example.com")
        )
        .await,
        Err(ReferralError::Rejected(ReferralRejection::BadDetails))
    ));
    assert_eq!(referral_balance(&db.store, referrer).await, usd(2000));
    let paypal = request_withdrawal(
        &ctx,
        referrer,
        &ask_with("paypal", "5", "  me@example.com  "),
    )
    .await
    .unwrap();
    assert_eq!(paypal.method, "paypal");
    assert_eq!(paypal.method_title, "PayPal");
    assert_eq!(paypal.details, "me@example.com");
    assert_eq!(paypal.amount, usd(500));
    assert_eq!(paypal.status, WithdrawalStatus::Pending);
    let usdt = request_withdrawal(&ctx, referrer, &ask_with("usdt_trc20", "2", wallet))
        .await
        .unwrap();
    assert_eq!(usdt.method_title, "USDT");
    assert_eq!(referral_balance(&db.store, referrer).await, usd(1300));
    configure(
        &db.store,
        serde_json::json!({ "payout_methods": [
            { "id": "paypal", "title": "PayPal (новый)", "check": "text" }
        ] }),
    )
    .await;
    let mut conn = db.store.conn().await.unwrap();
    let mine = store::client_withdrawals(&mut conn, referrer, 10)
        .await
        .unwrap();
    assert_eq!(
        mine.iter()
            .map(|w| (w.method.as_str(), w.method_title.as_str()))
            .collect::<Vec<_>>(),
        [("usdt_trc20", "USDT"), ("paypal", "PayPal")],
        "название способа берётся на момент заявки"
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn старые_заявки_показываются_по_встроенным_кодам() {
    let Some(db) = test_store().await else {
        return;
    };
    let referrer = client(&db.store).await;
    let mut conn = db.store.conn().await.unwrap();
    for (method, details) in [
        ("sbp", "+7 900 123-45-67, Т-Банк"),
        ("card", "4111111111111111"),
        ("usdt_trc20", "TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC"),
        ("bank_old", "счёт 40817810"),
    ] {
        sqlx::query(
            "INSERT INTO referral_withdrawals (client_id, amount_minor, currency, method, details, status) \
             VALUES ($1, 100, 'USD', $2, $3, 'pending')",
        )
        .bind(referrer.0)
        .bind(method)
        .bind(details)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let found = store::client_withdrawals(&mut conn, referrer, 10)
        .await
        .unwrap();
    assert_eq!(
        found
            .iter()
            .map(|w| (w.method.as_str(), w.method_title.as_str()))
            .collect::<Vec<_>>(),
        [
            ("bank_old", "bank_old"),
            ("usdt_trc20", "USDT (TRC20)"),
            ("card", "Банковская карта"),
            ("sbp", "СБП"),
        ]
    );
    let json = serde_json::to_value(&found[2]).unwrap();
    assert_eq!(json["method"], "card");
    assert_eq!(json["method_title"], "Банковская карта");
    drop(conn);
    db.cleanup().await;
}

async fn client_call(
    router: &axum::Router,
    request: axum::http::request::Builder,
    body: Option<serde_json::Value>,
    principal: &sn_domain::access::ClientPrincipal,
) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let request = request
        .header("content-type", "application/json")
        .extension(principal.clone())
        .body(match body {
            Some(body) => axum::body::Body::from(body.to_string()),
            None => axum::body::Body::empty(),
        })
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

#[tokio::test]
async fn клиентский_api_отдаёт_способы_вывода_и_название_в_заявках() {
    use axum::http::{Request, StatusCode};
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let registry =
        std::sync::Arc::new(crate::registry::ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.client_router(ctx.clone());
    let referrer = client(&db.store).await;
    move_referral(&db.store, referrer, 1000, "test:prize").await;
    let principal = sn_domain::access::ClientPrincipal {
        client_id: referrer,
        channel: sn_domain::access::ClientChannel::Cabinet,
        full_access: true,
        session_id: Some(1),
    };

    let (status, me) = client_call(&router, Request::get("/referral/me"), None, &principal).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["withdrawals_enabled"], true);
    assert_eq!(
        me["payout_methods"],
        serde_json::json!([
            { "id": "sbp", "title": "СБП", "hint": "Номер телефона и банк, например: +7 900 123-45-67, Т-Банк", "min": null },
            { "id": "card", "title": "Банковская карта", "hint": "Номер карты, 16 цифр", "min": null },
            { "id": "usdt_trc20", "title": "USDT (TRC20)", "hint": "Адрес кошелька TRC20, начинается с T", "min": null }
        ])
    );

    configure(&db.store, paypal_config()).await;
    let (_, me) = client_call(&router, Request::get("/referral/me"), None, &principal).await;
    assert_eq!(
        me["withdraw_min"],
        serde_json::json!({ "minor": 200, "currency": "USD" })
    );
    assert_eq!(
        me["payout_methods"],
        serde_json::json!([
            { "id": "paypal", "title": "PayPal", "hint": "Почта PayPal", "min": { "minor": 500, "currency": "USD" } },
            { "id": "usdt_trc20", "title": "USDT", "hint": "Адрес TRC20", "min": { "minor": 200, "currency": "USD" } }
        ])
    );
    let (status, created) = client_call(
        &router,
        Request::post("/referral/withdrawals"),
        Some(serde_json::json!({ "amount": "6", "method": "paypal", "details": "me@example.com" })),
        &principal,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["method"], "paypal");
    assert_eq!(created["method_title"], "PayPal");
    assert_eq!(created["details"], "me@example.com");
    let (status, refused) = client_call(
        &router,
        Request::post("/referral/withdrawals"),
        Some(serde_json::json!({ "amount": "3", "method": "sbp", "details": "+7 900 123-45-67" })),
        &principal,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused["error"]["code"], "referral.method_unavailable");
    let (status, low) = client_call(
        &router,
        Request::post("/referral/withdrawals"),
        Some(serde_json::json!({ "amount": "3", "method": "paypal", "details": "me@example.com" })),
        &principal,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(low["error"]["code"], "referral.below_minimum");
    assert_eq!(low["error"]["message"], "Минимальная сумма вывода — $5.00");
    let (_, me) = client_call(&router, Request::get("/referral/me"), None, &principal).await;
    assert_eq!(me["withdrawals"][0]["method"], "paypal");
    assert_eq!(me["withdrawals"][0]["method_title"], "PayPal");
    assert_eq!(me["balance"]["minor"], 400);

    configure(
        &db.store,
        serde_json::json!({ "withdrawals": false, "payout_methods": [] }),
    )
    .await;
    let (_, me) = client_call(&router, Request::get("/referral/me"), None, &principal).await;
    assert_eq!(me["withdrawals_enabled"], false);
    assert_eq!(me["payout_methods"], serde_json::json!([]));
    db.cleanup().await;
}
