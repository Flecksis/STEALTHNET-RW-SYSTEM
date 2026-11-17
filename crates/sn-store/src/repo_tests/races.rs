use super::fixtures::*;
use crate::testing::test_store;
use crate::*;
use sn_domain::ids::{ClientId, JobId, OrderId, PaymentId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::{LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::money::{Currency, Money};
use sn_domain::order::OrderStatus;
use sn_domain::payment::PaymentStatus;
use std::collections::HashSet;
use std::time::Duration;

fn entry(client: ClientId, minor: i64, kind: LedgerKind, key: String) -> NewLedgerEntry {
    NewLedgerEntry {
        client_id: client,
        wallet: Wallet::Main,
        amount: Money::new(minor, Currency::Usd),
        kind,
        order_id: None,
        payment_id: None,
        admin_id: None,
        comment: None,
        idempotency_key: Some(key),
    }
}

async fn append_in_tx(store: &Store, entry: &NewLedgerEntry) -> StoreResult<LedgerAppend> {
    let mut tx = store.begin().await?;
    match tx.ledger().append(entry).await {
        Ok(appended) => {
            tx.commit().await?;
            Ok(appended)
        }
        Err(err) => {
            tx.rollback().await?;
            Err(err)
        }
    }
}

async fn append_on_conn(store: &Store, entry: &NewLedgerEntry) -> StoreResult<LedgerAppend> {
    store.conn().await?.ledger().append(entry).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn два_параллельных_списания_не_уводят_баланс_в_минус() {
    let Some(db) = test_store().await else { return };
    let store = db.store.clone();
    let client = client(&mut store.conn().await.unwrap(), &Person::NOBODY).await;
    for round in 0..8 {
        append_on_conn(
            &store,
            &entry(client, 500, LedgerKind::TopUp, format!("topup:{round}")),
        )
        .await
        .unwrap();
        let first = entry(client, -300, LedgerKind::Purchase, format!("buy:{round}:a"));
        let second = entry(client, -300, LedgerKind::Purchase, format!("buy:{round}:b"));
        let (a, b) = if round % 2 == 0 {
            tokio::join!(append_in_tx(&store, &first), append_in_tx(&store, &second))
        } else {
            tokio::join!(
                append_on_conn(&store, &first),
                append_on_conn(&store, &second)
            )
        };
        let results = [a, b];
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            1,
            "раунд {round}"
        );
        assert!(
            results
                .iter()
                .any(|r| matches!(r, Err(StoreError::InsufficientFunds))),
            "раунд {round}"
        );
        let left = store
            .conn()
            .await
            .unwrap()
            .ledger()
            .balance(client, Wallet::Main, Currency::Usd)
            .await
            .unwrap();
        assert_eq!(left.minor, 200, "раунд {round}");
        append_on_conn(
            &store,
            &entry(client, -200, LedgerKind::Purchase, format!("rest:{round}")),
        )
        .await
        .unwrap();
    }
    let entries: i64 =
        sqlx::query_scalar("SELECT count(*) FROM balance_entries WHERE client_id = $1")
            .bind(client.0)
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(entries, 8 * 3);
    db.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn параллельное_зачисление_с_одним_ключом_проходит_один_раз() {
    let Some(db) = test_store().await else { return };
    let store = db.store.clone();
    let client = client(&mut store.conn().await.unwrap(), &Person::NOBODY).await;
    for round in 0..8 {
        let credit = entry(
            client,
            100,
            LedgerKind::TopUp,
            format!("topup:payment:{round}"),
        );
        let (a, b) = if round % 2 == 0 {
            tokio::join!(append_in_tx(&store, &credit), append_in_tx(&store, &credit))
        } else {
            tokio::join!(
                append_on_conn(&store, &credit),
                append_on_conn(&store, &credit)
            )
        };
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(a.entry_id, b.entry_id, "раунд {round}");
        assert_eq!(
            [a.duplicate, b.duplicate].iter().filter(|d| !**d).count(),
            1,
            "раунд {round}"
        );
    }
    let balance = store
        .conn()
        .await
        .unwrap()
        .ledger()
        .balance(client, Wallet::Main, Currency::Usd)
        .await
        .unwrap();
    assert_eq!(balance.minor, 800);
    db.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn две_параллельные_выборки_задач_не_берут_одну_задачу_дважды() {
    let Some(db) = test_store().await else { return };
    let store = db.store.clone();
    {
        let mut conn = store.conn().await.unwrap();
        for order in 1..=40 {
            conn.jobs()
                .enqueue(
                    &JobPayload::FulfillOrder {
                        order_id: OrderId(order),
                    },
                    None,
                )
                .await
                .unwrap()
                .unwrap();
        }
    }
    let claim = || async {
        store
            .conn()
            .await
            .unwrap()
            .jobs()
            .claim(25, Duration::from_secs(60))
            .await
            .unwrap()
    };
    let (a, b) = tokio::join!(claim(), claim());
    let mut seen: HashSet<JobId> = HashSet::new();
    for job in a.iter().chain(b.iter()) {
        assert!(seen.insert(job.id), "задача {} выдана дважды", job.id);
    }
    let rest = claim().await;
    for job in &rest {
        assert!(seen.insert(job.id), "задача {} выдана дважды", job.id);
    }
    assert_eq!(seen.len(), 40);
    assert!(claim().await.is_empty());
    db.cleanup().await;
}

async fn pay_in_tx(store: &Store, id: PaymentId) -> StoreResult<StatusChange> {
    let mut tx = store.begin().await?;
    let change = tx
        .payments()
        .transition(id, PaymentStatus::Paid, Some("tx-1"), None)
        .await?;
    tx.commit().await?;
    Ok(change)
}

async fn pay_on_conn(store: &Store, id: PaymentId) -> StoreResult<StatusChange> {
    store
        .conn()
        .await?
        .payments()
        .transition(id, PaymentStatus::Paid, Some("tx-1"), None)
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn два_параллельных_перевода_в_оплачено_применяются_ровно_один_раз() {
    let Some(db) = test_store().await else { return };
    let store = db.store.clone();
    let client = client(&mut store.conn().await.unwrap(), &Person::NOBODY).await;
    for round in 0..10 {
        let id = payment(
            &mut store.conn().await.unwrap(),
            client,
            &format!("p{round}"),
            usd(699),
        )
        .await;
        let (a, b) = if round % 2 == 0 {
            tokio::join!(pay_in_tx(&store, id), pay_on_conn(&store, id))
        } else {
            tokio::join!(pay_on_conn(&store, id), pay_on_conn(&store, id))
        };
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(
            [a.applied(), b.applied()].iter().filter(|x| **x).count(),
            1,
            "раунд {round}"
        );
        assert_eq!(a.payment().status, PaymentStatus::Paid);
        assert_eq!(b.payment().status, PaymentStatus::Paid);
        let order = a.payment().order_id;
        let mark = || async {
            store
                .conn()
                .await
                .unwrap()
                .orders()
                .transition(order, OrderStatus::Paid)
                .await
                .unwrap()
                .1
        };
        let (x, y) = tokio::join!(mark(), mark());
        assert!(x ^ y, "раунд {round}");
    }
    db.cleanup().await;
}
