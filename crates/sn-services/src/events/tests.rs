use crate::checkout::testkit::*;
use crate::clock::FixedClock;
use crate::services::{EnvFallbacks, ServiceDeps, Services};
use async_trait::async_trait;
use chrono::Duration;
use sn_domain::events::DomainEvent;
use sn_domain::ids::{ClientId, OrderId, PaymentId};
use sn_domain::module::ModuleId;
use sn_domain::money::{Amount, PayCurrency};
use sn_domain::order::OrderKind;
use sn_domain::payment::ProviderId;
use sn_modules::{Module, ModuleContext, ModuleError, ModuleRegistry};
use sn_payments::ProviderRegistry;
use sn_store::Db;
use sn_store::testing::{TestDb, test_store};
use std::sync::{Arc, Mutex};

struct Listener {
    seen: Mutex<Vec<&'static str>>,
}

#[async_trait]
impl Module for Listener {
    fn id(&self) -> ModuleId {
        ModuleId::Referral
    }

    async fn on_event(&self, _ctx: &ModuleContext, event: &DomainEvent) -> Result<(), ModuleError> {
        self.seen.lock().unwrap().push(event.kind());
        Ok(())
    }
}

#[tokio::test]
async fn события_оплаты_доходят_до_модулей_один_раз() {
    let listener = Arc::new(Listener {
        seen: Mutex::new(Vec::new()),
    });
    let Some(kit) = kit_with(vec![listener.clone()]).await else {
        return;
    };
    let client_id = client(&kit.services, 601).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    let events = kit.services.events();
    let dispatched = events.dispatch_pending(100).await.unwrap();
    assert!(dispatched >= 4, "{dispatched}");
    assert_eq!(events.dispatch_pending(100).await.unwrap(), 0);
    let seen = listener.seen.lock().unwrap().clone();
    assert_eq!(
        seen.iter().filter(|kind| **kind == "payment_paid").count(),
        1
    );
    assert!(seen.contains(&"balance_changed"));
    assert!(seen.contains(&"client_registered"));
    let mut conn = kit.services.store().conn().await.unwrap();
    assert!(conn.events().claim_pending(10).await.unwrap().is_empty());
    drop(conn);
    kit.db.cleanup().await;
}

struct Flaky {
    id: ModuleId,
    failures: Mutex<u32>,
    seen: Mutex<Vec<(&'static str, ClientId)>>,
}

impl Flaky {
    fn new(id: ModuleId, failures: u32) -> Arc<Self> {
        Arc::new(Flaky {
            id,
            failures: Mutex::new(failures),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn seen(&self) -> Vec<(&'static str, ClientId)> {
        self.seen.lock().unwrap().clone()
    }

    fn heal(&self) {
        *self.failures.lock().unwrap() = 0;
    }
}

#[async_trait]
impl Module for Flaky {
    fn id(&self) -> ModuleId {
        self.id
    }

    async fn on_event(&self, _ctx: &ModuleContext, event: &DomainEvent) -> Result<(), ModuleError> {
        self.seen
            .lock()
            .unwrap()
            .push((event.kind(), event.client_id()));
        let mut failures = self.failures.lock().unwrap();
        if *failures > 0 {
            *failures -= 1;
            return Err(ModuleError::Core("временный сбой".into()));
        }
        Ok(())
    }
}

struct Clocked {
    db: TestDb,
    services: Services,
    clock: Arc<FixedClock>,
}

impl Clocked {
    fn advance(&self, seconds: i64) {
        let now = self.services.now();
        self.clock.set(now + Duration::seconds(seconds));
    }
}

async fn clocked(modules: Vec<Arc<dyn Module>>) -> Option<Clocked> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(moment(Duration::zero())));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(Vec::new())),
        modules: Arc::new(ModuleRegistry::new(modules).unwrap()),
        secrets: None,
        clock: clock.clone(),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    Some(Clocked {
        db,
        services,
        clock,
    })
}

fn paid(client: i64) -> DomainEvent {
    DomainEvent::PaymentPaid {
        payment_id: PaymentId(client * 10),
        order_id: OrderId(client * 10),
        client_id: ClientId(client),
        order_kind: OrderKind::New,
        amount: Amount::new(299, PayCurrency::Usd),
        provider: ProviderId::from("testpay"),
    }
}

fn registered(client: i64) -> DomainEvent {
    DomainEvent::ClientRegistered {
        client_id: ClientId(client),
        source: sn_domain::client::RegistrationSource::Bot,
        start_param: Some("ref_abc".into()),
    }
}

async fn push(services: &Services, event: &DomainEvent) {
    let mut conn = services.store().conn().await.unwrap();
    conn.events().push(event).await.unwrap();
}

async fn pending_count(services: &Services) -> usize {
    let mut conn = services.store().conn().await.unwrap();
    conn.events().claim_pending(100).await.unwrap().len()
}

#[tokio::test]
async fn модуль_с_ошибкой_получает_событие_повторно_после_паузы() {
    let flaky = Flaky::new(ModuleId::Referral, 1);
    let steady = Flaky::new(ModuleId::Utm, 0);
    let Some(env) = clocked(vec![flaky.clone(), steady.clone()]).await else {
        return;
    };
    let events = env.services.events();
    push(&env.services, &paid(7)).await;
    assert_eq!(events.dispatch_pending(100).await.unwrap(), 1);
    assert_eq!(flaky.seen().len(), 1);
    assert_eq!(steady.seen().len(), 1);
    assert_eq!(pending_count(&env.services).await, 1);
    events.dispatch_pending(100).await.unwrap();
    assert_eq!(flaky.seen().len(), 1, "до паузы повтора нет");
    env.advance(10);
    events.dispatch_pending(100).await.unwrap();
    assert_eq!(
        flaky.seen(),
        vec![("payment_paid", ClientId(7)), ("payment_paid", ClientId(7))]
    );
    assert_eq!(steady.seen().len(), 1, "успешный модуль не получает повтор");
    assert_eq!(pending_count(&env.services).await, 0);
    env.advance(3_600);
    assert_eq!(events.dispatch_pending(100).await.unwrap(), 0);
    assert_eq!(flaky.seen().len(), 2);
    env.db.cleanup().await;
}

#[tokio::test]
async fn события_клиента_доходят_до_модуля_по_порядку() {
    let flaky = Flaky::new(ModuleId::Referral, 1);
    let steady = Flaky::new(ModuleId::Utm, 0);
    let Some(env) = clocked(vec![flaky.clone(), steady.clone()]).await else {
        return;
    };
    let events = env.services.events();
    push(&env.services, &registered(9)).await;
    push(&env.services, &paid(9)).await;
    push(&env.services, &paid(10)).await;
    events.dispatch_pending(100).await.unwrap();
    assert_eq!(
        flaky.seen(),
        vec![
            ("client_registered", ClientId(9)),
            ("payment_paid", ClientId(10))
        ],
        "оплата клиента 9 ждёт, пока модуль не примет его регистрацию"
    );
    assert_eq!(steady.seen().len(), 3);
    env.advance(10);
    events.dispatch_pending(100).await.unwrap();
    assert_eq!(
        flaky.seen(),
        vec![
            ("client_registered", ClientId(9)),
            ("payment_paid", ClientId(10)),
            ("client_registered", ClientId(9)),
            ("payment_paid", ClientId(9))
        ]
    );
    assert_eq!(steady.seen().len(), 3);
    assert_eq!(pending_count(&env.services).await, 0);
    env.db.cleanup().await;
}

#[tokio::test]
async fn исчерпанная_доставка_уходит_на_ручную_проверку() {
    let flaky = Flaky::new(ModuleId::Referral, u32::MAX);
    let Some(env) = clocked(vec![flaky.clone()]).await else {
        return;
    };
    let events = env.services.events();
    push(&env.services, &paid(11)).await;
    for _ in 0..40 {
        events.dispatch_pending(100).await.unwrap();
        env.advance(7_200);
    }
    assert_eq!(
        flaky.seen().len(),
        crate::events::MAX_DELIVERY_ATTEMPTS as usize
    );
    assert_eq!(pending_count(&env.services).await, 0);
    let review = events.review(10).await.unwrap();
    assert_eq!(review.len(), 1);
    assert_eq!(review[0].module, ModuleId::Referral);
    assert_eq!(review[0].attempts, crate::events::MAX_DELIVERY_ATTEMPTS);
    assert!(
        review[0]
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("временный сбой"))
    );
    let audit: i64 = {
        let mut conn = env.services.store().conn().await.unwrap();
        sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action = 'module.event_review' AND entity_id = $1",
        )
        .bind(review[0].event_id.0)
        .fetch_one(conn.conn())
        .await
        .unwrap()
    };
    assert_eq!(audit, 1);
    flaky.heal();
    assert!(
        events
            .retry(review[0].event_id, ModuleId::Referral)
            .await
            .unwrap()
    );
    assert_eq!(events.dispatch_pending(100).await.unwrap(), 1);
    assert_eq!(
        flaky.seen().len(),
        crate::events::MAX_DELIVERY_ATTEMPTS as usize + 1
    );
    assert!(events.review(10).await.unwrap().is_empty());
    assert_eq!(pending_count(&env.services).await, 0);
    env.db.cleanup().await;
}

#[tokio::test]
async fn выключенный_модуль_не_держит_событие() {
    let flaky = Flaky::new(ModuleId::Referral, 1);
    let Some(env) = clocked(vec![flaky.clone()]).await else {
        return;
    };
    let events = env.services.events();
    push(&env.services, &paid(12)).await;
    events.dispatch_pending(100).await.unwrap();
    assert_eq!(pending_count(&env.services).await, 1);
    env.services
        .modules()
        .set_enabled(env.services.store(), ModuleId::Referral, false, None)
        .await
        .unwrap();
    env.advance(10);
    events.dispatch_pending(100).await.unwrap();
    assert_eq!(flaky.seen().len(), 1);
    assert_eq!(pending_count(&env.services).await, 0);
    env.db.cleanup().await;
}
