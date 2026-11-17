use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::gate::{GateState, gate};
use crate::module::{Module, ModuleConfig, QuoteHook, QuoteOrigin};
use crate::placeholder::Placeholder;
use axum::Router;
use axum::middleware::from_fn_with_state;
use serde::Serialize;
use sn_domain::events::DomainEvent;
use sn_domain::ids::AdminId;
use sn_domain::module::{MenuEntry, ModuleId, ModuleInfo};
use sn_domain::money::Currency;
use sn_domain::order::Order;
use sn_domain::payment::Payment;
use sn_domain::pricing::PriceAdjustment;
use sn_store::{Db, ModuleStateRow, Store, Tx};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Default, PartialEq)]
struct ModuleState {
    enabled: bool,
    config: ModuleConfig,
}

struct State {
    currency: Currency,
    modules: Vec<ModuleState>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModuleView {
    pub info: &'static ModuleInfo,
    pub enabled: bool,
    pub active: bool,
    pub locked: Option<&'static str>,
    pub warning: Option<&'static str>,
    pub has_settings: bool,
}

pub struct ModuleRegistry {
    modules: Vec<Arc<dyn Module>>,
    state: RwLock<State>,
}

const CURRENCY_LOCK: &str = "Только при валюте „Рубли“";
const NEEDS_KEY: &str = "Нужен API-ключ";

impl ModuleRegistry {
    pub fn new(implementations: Vec<Arc<dyn Module>>) -> Result<Self, ModuleError> {
        let mut slots: Vec<Option<Arc<dyn Module>>> = vec![None; ModuleId::ALL.len()];
        for module in implementations {
            let index = module.id() as usize;
            if slots[index].is_some() {
                return Err(ModuleError::Duplicate(module.id()));
            }
            slots[index] = Some(module);
        }
        let modules: Vec<Arc<dyn Module>> = ModuleId::ALL
            .into_iter()
            .zip(slots)
            .map(|(id, slot)| slot.unwrap_or_else(|| Arc::new(Placeholder(id))))
            .collect();
        let states = ModuleId::ALL
            .into_iter()
            .map(|id| ModuleState {
                enabled: id.info().enabled_by_default,
                config: ModuleConfig::default(),
            })
            .collect();
        Ok(ModuleRegistry {
            modules,
            state: RwLock::new(State {
                currency: Currency::Usd,
                modules: states,
            }),
        })
    }

    pub fn catalog_only() -> Self {
        Self {
            modules: ModuleId::ALL
                .into_iter()
                .map(|id| Arc::new(Placeholder(id)) as Arc<dyn Module>)
                .collect(),
            state: RwLock::new(State {
                currency: Currency::Usd,
                modules: ModuleId::ALL
                    .into_iter()
                    .map(|id| ModuleState {
                        enabled: id.info().enabled_by_default,
                        config: ModuleConfig::default(),
                    })
                    .collect(),
            }),
        }
    }

    fn read<T>(&self, f: impl FnOnce(&State) -> T) -> T {
        match self.state.read() {
            Ok(guard) => f(&guard),
            Err(poisoned) => f(&poisoned.into_inner()),
        }
    }

    fn write<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        match self.state.write() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => f(&mut poisoned.into_inner()),
        }
    }

    pub fn get(&self, id: ModuleId) -> Arc<dyn Module> {
        self.modules[id as usize].clone()
    }

    pub fn currency(&self) -> Currency {
        self.read(|s| s.currency)
    }

    pub fn set_currency(&self, currency: Currency) {
        self.write(|s| s.currency = currency);
    }

    pub fn is_enabled(&self, id: ModuleId) -> bool {
        self.read(|s| s.modules[id as usize].enabled && id.available_in(s.currency))
    }

    pub fn config(&self, id: ModuleId) -> ModuleConfig {
        self.read(|s| s.modules[id as usize].config.clone())
    }

    pub fn enabled(&self) -> Vec<Arc<dyn Module>> {
        ModuleId::ALL
            .into_iter()
            .filter(|id| self.is_enabled(*id))
            .map(|id| self.get(id))
            .collect()
    }

    pub fn views(&self) -> Vec<ModuleView> {
        self.read(|s| {
            ModuleId::ALL
                .into_iter()
                .map(|id| {
                    let info = id.info();
                    let state = &s.modules[id as usize];
                    let locked = (!id.available_in(s.currency)).then_some(CURRENCY_LOCK);
                    let active = state.enabled && locked.is_none();
                    let module = &self.modules[id as usize];
                    let schema = module.settings_schema();
                    let warning = (active
                        && info.needs_api_key
                        && module.validate_config(&state.config).is_err())
                    .then_some(NEEDS_KEY);
                    ModuleView {
                        info,
                        enabled: state.enabled,
                        active,
                        locked,
                        warning,
                        has_settings: !schema.is_empty(),
                    }
                })
                .collect()
        })
    }

    pub fn menu(&self) -> Vec<MenuEntry> {
        let mut entries: Vec<MenuEntry> = ModuleId::ALL
            .into_iter()
            .filter(|id| self.is_enabled(*id))
            .filter_map(|id| id.info().menu)
            .collect();
        entries.sort_by_key(|e| e.order);
        entries
    }

    pub fn apply_rows(&self, rows: &[ModuleStateRow]) {
        self.write(|s| {
            for row in rows {
                if let Ok(id) = ModuleId::parse(&row.module) {
                    s.modules[id as usize] = ModuleState {
                        enabled: row.enabled,
                        config: ModuleConfig::from_value(&row.config),
                    };
                }
            }
        });
    }

    pub async fn load(&self, store: &Store) -> Result<(), ModuleError> {
        let defaults: Vec<(&str, bool)> = ModuleId::ALL
            .into_iter()
            .map(|id| (id.as_str(), id.info().enabled_by_default))
            .collect();
        let mut conn = store.conn().await?;
        conn.modules().ensure_defaults(&defaults).await?;
        let rows = conn.modules().all().await?;
        self.apply_rows(&rows);
        Ok(())
    }

    pub async fn set_enabled(
        &self,
        store: &Store,
        id: ModuleId,
        enabled: bool,
        by: Option<AdminId>,
    ) -> Result<ModuleView, ModuleError> {
        if enabled && !id.available_in(self.currency()) {
            return Err(ModuleError::Unavailable {
                module: id,
                reason: CURRENCY_LOCK.into(),
            });
        }
        let mut conn = store.conn().await?;
        let row = conn.modules().set_enabled(id.as_str(), enabled, by).await?;
        self.apply_rows(std::slice::from_ref(&row));
        self.views()
            .into_iter()
            .find(|v| v.info.id == id)
            .ok_or(ModuleError::NotFound("модуль"))
    }

    pub async fn set_config(
        &self,
        store: &Store,
        id: ModuleId,
        config: ModuleConfig,
        by: Option<AdminId>,
    ) -> Result<(), ModuleError> {
        self.get(id).validate_config(&config)?;
        let mut conn = store.conn().await?;
        let row = conn
            .modules()
            .set_config(id.as_str(), &config.to_value(), by)
            .await?;
        self.apply_rows(std::slice::from_ref(&row));
        Ok(())
    }

    pub fn recipients(&self) -> Vec<ModuleId> {
        ModuleId::ALL
            .into_iter()
            .filter(|id| self.is_enabled(*id))
            .collect()
    }

    pub async fn deliver(
        &self,
        ctx: &ModuleContext,
        id: ModuleId,
        event: &DomainEvent,
    ) -> Result<(), ModuleError> {
        if !self.is_enabled(id) {
            return Err(ModuleError::Disabled(id));
        }
        self.get(id).on_event(ctx, event).await
    }

    pub async fn order_created(
        &self,
        ctx: &ModuleContext,
        tx: &mut Tx,
        order: &Order,
        payment: &Payment,
    ) -> Result<(), ModuleError> {
        for module in self.enabled() {
            module.on_order_created(ctx, tx, order, payment).await?;
        }
        Ok(())
    }

    pub async fn order_paid(
        &self,
        ctx: &ModuleContext,
        tx: &mut Tx,
        order: &Order,
        payment: &Payment,
    ) -> Result<(), ModuleError> {
        for module in self.enabled() {
            module.on_order_paid(ctx, tx, order, payment).await?;
        }
        Ok(())
    }

    pub async fn order_canceled(
        &self,
        ctx: &ModuleContext,
        tx: &mut Tx,
        order: &Order,
    ) -> Result<(), ModuleError> {
        for module in self.enabled() {
            module.on_order_canceled(ctx, tx, order).await?;
        }
        Ok(())
    }

    pub async fn quote_adjustments(
        &self,
        ctx: &ModuleContext,
        hook: &QuoteHook,
    ) -> Result<Vec<PriceAdjustment>, ModuleError> {
        self.quote_adjustments_from(ctx, hook, &QuoteOrigin::default())
            .await
    }

    pub async fn quote_adjustments_from(
        &self,
        ctx: &ModuleContext,
        hook: &QuoteHook,
        origin: &QuoteOrigin,
    ) -> Result<Vec<PriceAdjustment>, ModuleError> {
        let mut adjustments = Vec::new();
        for module in self.enabled() {
            adjustments.extend(module.quote_adjustments_from(ctx, hook, origin).await?);
        }
        Ok(adjustments)
    }

    fn mount(self: &Arc<Self>, routes: impl Fn(&dyn Module) -> Option<Router>) -> Router {
        let mut router = Router::new();
        for module in &self.modules {
            if let Some(module_router) = routes(module.as_ref()) {
                let gate_state = GateState {
                    registry: self.clone(),
                    module: module.id(),
                };
                router = router.nest(
                    &format!("/{}", module.id().as_str()),
                    module_router.layer(from_fn_with_state(gate_state, gate)),
                );
            }
        }
        router
    }

    pub fn admin_router(self: &Arc<Self>, ctx: ModuleContext) -> Router {
        self.mount(|m| m.admin_routes(ctx.clone()))
    }

    pub fn client_router(self: &Arc<Self>, ctx: ModuleContext) -> Router {
        self.mount(|m| m.client_routes(ctx.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{BalanceChange, CoreApi, Notification};
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use sn_domain::ids::{ClientId, SubscriptionId};
    use sn_domain::jobs::JobPayload;
    use sn_domain::money::Money;
    use tower::ServiceExt;

    struct Ping;

    #[async_trait]
    impl Module for Ping {
        fn id(&self) -> ModuleId {
            ModuleId::Gifts
        }

        fn admin_routes(&self, _ctx: ModuleContext) -> Option<Router> {
            Some(Router::new().route("/ping", get(|| async { "pong" })))
        }
    }

    struct NoCore;

    #[async_trait]
    impl CoreApi for NoCore {
        async fn change_balance(&self, _change: BalanceChange) -> Result<Money, ModuleError> {
            Err(ModuleError::NotImplemented("change_balance"))
        }

        async fn add_days(
            &self,
            _id: SubscriptionId,
            _days: u32,
            _reason: &str,
        ) -> Result<(), ModuleError> {
            Err(ModuleError::NotImplemented("add_days"))
        }

        async fn notify(
            &self,
            _client_id: ClientId,
            _notification: Notification,
        ) -> Result<(), ModuleError> {
            Err(ModuleError::NotImplemented("notify"))
        }

        async fn enqueue(&self, _job: JobPayload) -> Result<(), ModuleError> {
            Err(ModuleError::NotImplemented("enqueue"))
        }
    }

    fn context() -> ModuleContext {
        ModuleContext {
            store: Store::connect_lazy("postgres://localhost/sn_test", Default::default()).unwrap(),
            core: Arc::new(NoCore),
            currency: Currency::Usd,
        }
    }

    #[test]
    fn мой_налог_заблокирован_при_долларах() {
        let registry = ModuleRegistry::catalog_only();
        let nalog = registry
            .views()
            .into_iter()
            .find(|v| v.info.id == ModuleId::Nalog)
            .unwrap();
        assert_eq!(nalog.locked, Some(CURRENCY_LOCK));
        registry.apply_rows(&[ModuleStateRow {
            module: "nalog".into(),
            enabled: true,
            config: serde_json::json!({}),
            updated_at: chrono::Utc::now(),
        }]);
        assert!(!registry.is_enabled(ModuleId::Nalog));
        registry.set_currency(Currency::Rub);
        assert!(registry.is_enabled(ModuleId::Nalog));
    }

    #[test]
    fn меню_строится_только_из_включённых_модулей() {
        let registry = ModuleRegistry::catalog_only();
        let labels: Vec<&str> = registry.menu().into_iter().map(|e| e.label).collect();
        assert_eq!(labels, ["Рассылки", "Промокоды", "Рефералы"]);
    }

    struct AiChat;

    #[async_trait]
    impl Module for AiChat {
        fn id(&self) -> ModuleId {
            ModuleId::Ai
        }

        fn settings_schema(&self) -> Vec<sn_domain::settings::SettingField> {
            vec![sn_domain::settings::SettingField::secret(
                "groq_api_key",
                "API-ключ Groq",
                "console.groq.com → API Keys",
            )]
        }
    }

    #[test]
    fn ии_чат_без_ключа_показывает_предупреждение() {
        let registry = ModuleRegistry::new(vec![Arc::new(AiChat)]).unwrap();
        let row = |config: serde_json::Value| ModuleStateRow {
            module: "ai".into(),
            enabled: true,
            config,
            updated_at: chrono::Utc::now(),
        };
        let warning = |registry: &ModuleRegistry| {
            registry
                .views()
                .into_iter()
                .find(|v| v.info.id == ModuleId::Ai)
                .and_then(|v| v.warning)
        };
        registry.apply_rows(&[row(serde_json::json!({}))]);
        assert_eq!(warning(&registry), Some(NEEDS_KEY));
        registry.apply_rows(&[row(serde_json::json!({"groq_api_key": "gsk_test"}))]);
        assert_eq!(warning(&registry), None);
    }

    #[test]
    fn один_модуль_нельзя_зарегистрировать_дважды() {
        assert!(matches!(
            ModuleRegistry::new(vec![Arc::new(Ping), Arc::new(Ping)]),
            Err(ModuleError::Duplicate(ModuleId::Gifts))
        ));
    }

    #[tokio::test]
    async fn выключенный_модуль_закрывает_свои_маршруты_без_перезапуска() {
        let registry = Arc::new(ModuleRegistry::new(vec![Arc::new(Ping)]).unwrap());
        let router = registry.admin_router(context());
        let request = || Request::get("/gifts/ping").body(Body::empty()).unwrap();
        let response = router.clone().oneshot(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        registry.apply_rows(&[ModuleStateRow {
            module: "gifts".into(),
            enabled: true,
            config: serde_json::json!({}),
            updated_at: chrono::Utc::now(),
        }]);
        let response = router.oneshot(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn состояние_модулей_хранится_в_базе() {
        let Some(db) = sn_store::testing::test_store().await else {
            return;
        };
        let registry = ModuleRegistry::catalog_only();
        registry.load(&db.store).await.unwrap();
        assert!(registry.is_enabled(ModuleId::Promo));
        assert!(!registry.is_enabled(ModuleId::Gifts));
        registry
            .set_enabled(&db.store, ModuleId::Gifts, true, None)
            .await
            .unwrap();
        assert!(matches!(
            registry
                .set_enabled(&db.store, ModuleId::Nalog, true, None)
                .await,
            Err(ModuleError::Unavailable { .. })
        ));
        let fresh = ModuleRegistry::catalog_only();
        fresh.load(&db.store).await.unwrap();
        assert!(fresh.is_enabled(ModuleId::Gifts));
        db.cleanup().await;
    }

    struct Veto;

    #[async_trait]
    impl Module for Veto {
        fn id(&self) -> ModuleId {
            ModuleId::Gifts
        }

        async fn on_event(
            &self,
            _ctx: &ModuleContext,
            _event: &DomainEvent,
        ) -> Result<(), ModuleError> {
            Err(ModuleError::Core("не принято".into()))
        }

        async fn on_order_created(
            &self,
            _ctx: &ModuleContext,
            _tx: &mut Tx,
            _order: &Order,
            _payment: &Payment,
        ) -> Result<(), ModuleError> {
            Err(ModuleError::Invalid("заказ отклонён модулем".into()))
        }
    }

    #[tokio::test]
    async fn события_и_хуки_заказа_получают_только_включённые_модули() {
        let Some(db) = sn_store::testing::test_store().await else {
            return;
        };
        let registry = ModuleRegistry::new(vec![Arc::new(Veto)]).unwrap();
        let ctx = ModuleContext {
            store: db.store.clone(),
            core: Arc::new(NoCore),
            currency: Currency::Usd,
        };
        let event = DomainEvent::ClientUnblocked {
            client_id: ClientId(1),
        };
        assert!(!registry.recipients().contains(&ModuleId::Gifts));
        assert!(matches!(
            registry.deliver(&ctx, ModuleId::Gifts, &event).await,
            Err(ModuleError::Disabled(ModuleId::Gifts))
        ));
        let client = {
            let mut conn = db.store.conn().await.unwrap();
            conn.clients()
                .create(&sn_store::NewClient {
                    locale: sn_domain::client::Locale::Ru,
                    registration_source: sn_domain::client::RegistrationSource::Bot,
                    note: None,
                })
                .await
                .unwrap()
                .id
        };
        let mut tx = db.store.begin().await.unwrap();
        let currency = Currency::Usd;
        let order = tx
            .orders()
            .create(&sn_domain::order::NewOrder {
                client_id: client,
                subscription_id: None,
                kind: sn_domain::order::OrderKind::TopUp,
                intent: sn_domain::order::PurchaseIntent::TopUp {
                    amount: Money::new(500, currency),
                },
                quote: sn_domain::pricing::topup_quote(
                    Money::new(500, currency),
                    sn_domain::pricing::TopUpLimits {
                        min: Money::new(100, currency),
                        max: Money::new(100_000, currency),
                    },
                )
                .unwrap(),
                snapshot: None,
            })
            .await
            .unwrap();
        let payment = tx
            .payments()
            .create(&sn_domain::payment::NewPayment {
                order_id: order.id,
                client_id: client,
                provider: sn_domain::payment::ProviderId::from("cryptopay"),
                method: None,
                amount: Money::new(500, currency).into(),
                expires_at: None,
            })
            .await
            .unwrap();
        registry
            .order_created(&ctx, &mut tx, &order, &payment)
            .await
            .unwrap();
        registry
            .set_enabled(&db.store, ModuleId::Gifts, true, None)
            .await
            .unwrap();
        assert!(registry.recipients().contains(&ModuleId::Gifts));
        assert!(matches!(
            registry
                .order_created(&ctx, &mut tx, &order, &payment)
                .await,
            Err(ModuleError::Invalid(_))
        ));
        assert!(matches!(
            registry.deliver(&ctx, ModuleId::Gifts, &event).await,
            Err(ModuleError::Core(_))
        ));
        tx.rollback().await.unwrap();
        db.cleanup().await;
    }
}
