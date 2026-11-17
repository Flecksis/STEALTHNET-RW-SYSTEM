use crate::context::{BalanceChange, CoreApi, ModuleContext, Notification};
use crate::error::ModuleError;
use async_trait::async_trait;
use sn_domain::client::{Locale, RegistrationSource};
use sn_domain::ids::{ClientId, OrderId, SubscriptionId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::NewLedgerEntry;
use sn_domain::money::{Amount, Currency, Money};
use sn_domain::order::{NewOrder, Order, OrderKind, PurchaseIntent};
use sn_domain::payment::{NewPayment, Payment, PaymentStatus, ProviderId};
use sn_domain::pricing::Quote;
use sn_store::testing::TestDb;
use sn_store::{Db, NewClient, Store, StoreError, StoreOptions};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

#[derive(Default)]
pub struct Calls {
    pub days: Vec<(SubscriptionId, u32, String)>,
    pub addons: Vec<(OrderId, SubscriptionId, String, u64)>,
    pub notifications: Vec<(ClientId, Notification)>,
    pub jobs: Vec<JobPayload>,
}

pub struct TestCore {
    pub store: Store,
    pub calls: Mutex<Calls>,
    pub fail_days: AtomicBool,
    pub slow_days_ms: AtomicU64,
}

impl TestCore {
    pub fn new(store: &Store) -> Arc<Self> {
        Arc::new(TestCore {
            store: store.clone(),
            calls: Mutex::new(Calls::default()),
            fail_days: AtomicBool::new(false),
            slow_days_ms: AtomicU64::new(0),
        })
    }

    pub fn days(&self) -> Vec<(SubscriptionId, u32, String)> {
        self.calls
            .lock()
            .map(|c| c.days.clone())
            .unwrap_or_default()
    }

    pub fn addons(&self) -> Vec<(OrderId, SubscriptionId, String, u64)> {
        self.calls
            .lock()
            .map(|c| c.addons.clone())
            .unwrap_or_default()
    }

    pub fn notifications(&self) -> Vec<(ClientId, Notification)> {
        self.calls
            .lock()
            .map(|c| c.notifications.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl CoreApi for TestCore {
    async fn change_balance(&self, change: BalanceChange) -> Result<Money, ModuleError> {
        let mut tx = self.store.begin().await?;
        let appended = tx
            .ledger()
            .append(&NewLedgerEntry {
                client_id: change.client_id,
                wallet: change.wallet,
                amount: change.amount,
                kind: change.kind,
                order_id: change.order_id,
                payment_id: change.payment_id,
                admin_id: None,
                comment: change.comment.clone(),
                idempotency_key: Some(change.idempotency_key.clone()),
            })
            .await
            .map_err(|err| match err {
                StoreError::InsufficientFunds => {
                    ModuleError::Core("на балансе не хватает средств".into())
                }
                other => ModuleError::Store(other),
            })?;
        tx.commit().await?;
        Ok(appended.balance_after)
    }

    async fn add_days(
        &self,
        subscription_id: SubscriptionId,
        days: u32,
        reason: &str,
    ) -> Result<(), ModuleError> {
        if self.fail_days.load(Ordering::SeqCst) {
            return Err(ModuleError::Core("Remnawave недоступна".into()));
        }
        if let Ok(mut calls) = self.calls.lock() {
            calls.days.push((subscription_id, days, reason.to_string()));
        }
        let slow = self.slow_days_ms.load(Ordering::SeqCst);
        if slow > 0 {
            tokio::time::sleep(Duration::from_millis(slow)).await;
        }
        Ok(())
    }

    async fn notify(
        &self,
        client_id: ClientId,
        notification: Notification,
    ) -> Result<(), ModuleError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.notifications.push((client_id, notification));
        }
        Ok(())
    }

    async fn enqueue(&self, job: JobPayload) -> Result<(), ModuleError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.jobs.push(job);
        }
        Ok(())
    }

    async fn apply_device_addon(
        &self,
        order_id: OrderId,
        subscription_id: SubscriptionId,
        devices_after: u32,
    ) -> Result<(), ModuleError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.addons.push((
                order_id,
                subscription_id,
                "devices".into(),
                u64::from(devices_after),
            ));
        }
        Ok(())
    }

    async fn apply_traffic_addon(
        &self,
        order_id: OrderId,
        subscription_id: SubscriptionId,
        bytes: u64,
    ) -> Result<(), ModuleError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls
                .addons
                .push((order_id, subscription_id, "traffic".into(), bytes));
        }
        Ok(())
    }
}

pub fn context(store: &Store, core: &Arc<TestCore>, currency: Currency) -> ModuleContext {
    ModuleContext {
        store: store.clone(),
        core: core.clone(),
        currency,
    }
}

pub async fn pooled(db: &TestDb, connections: u32, acquire_timeout: Duration) -> Store {
    let url = sn_store::testing::test_database_url().unwrap();
    let schema: String = {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query_scalar("SELECT current_schema()")
            .fetch_one(conn.conn())
            .await
            .unwrap()
    };
    Store::connect(
        &url,
        StoreOptions {
            max_connections: connections,
            acquire_timeout,
            search_path: Some(schema),
            ..StoreOptions::default()
        },
    )
    .await
    .unwrap()
}

pub fn offline() -> Store {
    Store::connect_lazy(
        "postgres://127.0.0.1:1/sn_test",
        StoreOptions {
            acquire_timeout: Duration::from_secs(1),
            ..StoreOptions::default()
        },
    )
    .unwrap()
}

pub async fn client(store: &Store) -> ClientId {
    let mut conn = store.conn().await.unwrap();
    conn.clients()
        .create(&NewClient {
            locale: Locale::Ru,
            registration_source: RegistrationSource::Bot,
            note: None,
        })
        .await
        .unwrap()
        .id
}

pub async fn order(
    store: &Store,
    client_id: ClientId,
    kind: OrderKind,
    intent: PurchaseIntent,
    quote: Quote,
) -> Order {
    let mut conn = store.conn().await.unwrap();
    conn.orders()
        .create(&NewOrder {
            client_id,
            subscription_id: None,
            kind,
            intent,
            quote,
            snapshot: None,
        })
        .await
        .unwrap()
}

pub async fn paid_payment(
    store: &Store,
    order_id: OrderId,
    client_id: ClientId,
    provider: &str,
    amount: Amount,
) -> Payment {
    let mut conn = store.conn().await.unwrap();
    let payment = conn
        .payments()
        .create(&NewPayment {
            order_id,
            client_id,
            provider: ProviderId::from(provider),
            method: None,
            amount,
            expires_at: None,
        })
        .await
        .unwrap();
    conn.payments()
        .transition(
            payment.id,
            PaymentStatus::Paid,
            Some(&format!("tx-{}", payment.id)),
            None,
        )
        .await
        .unwrap()
        .payment()
        .clone()
}

pub fn usd(minor: i64) -> Money {
    Money::new(minor, Currency::Usd)
}
