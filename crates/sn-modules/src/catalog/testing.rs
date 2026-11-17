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
use sn_store::{Db, NewClient, Store, StoreError};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub struct Calls {
    pub days: Vec<(SubscriptionId, u32, String)>,
    pub notifications: Vec<(ClientId, Notification)>,
    pub jobs: Vec<JobPayload>,
}

pub struct TestCore {
    pub store: Store,
    pub calls: Mutex<Calls>,
    pub fail_days: AtomicBool,
}

impl TestCore {
    pub fn new(store: &Store) -> Arc<Self> {
        Arc::new(TestCore {
            store: store.clone(),
            calls: Mutex::new(Calls::default()),
            fail_days: AtomicBool::new(false),
        })
    }

    pub fn days(&self) -> Vec<(SubscriptionId, u32, String)> {
        self.calls
            .lock()
            .map(|c| c.days.clone())
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
}

pub fn context(store: &Store, core: &Arc<TestCore>, currency: Currency) -> ModuleContext {
    ModuleContext {
        store: store.clone(),
        core: core.clone(),
        currency,
    }
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
