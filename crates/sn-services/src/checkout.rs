use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::DomainError;
use sn_domain::client::{Client, IdentityKind};
use sn_domain::events::DomainEvent;
use sn_domain::format;
use sn_domain::ids::{CategoryId, ClientId, OrderId, PeriodId, SubscriptionId, TariffId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::{LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::module::ModuleId;
use sn_domain::money::{Amount, Currency, Money, PayCurrency};
use sn_domain::order::{NewOrder, Order, OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::payment::{NewPayment, Payment, PaymentStatus, ProviderId, providers};
use sn_domain::plan::{CurrentTariff, PlanKind, classify, new_expire_at};
use sn_domain::pricing::{Quote, QuoteRequest, quote, topup_quote};
use sn_domain::settings::TrialSettings;
use sn_domain::subscription::{Subscription, SubscriptionKind, SubscriptionStatus};
use sn_domain::tariff::{Category, PeriodOffer, Tariff, TariffSnapshot, period_offers};
use sn_modules::{ItemHook, ModuleContext, ModuleRegistry, QuoteHook, QuoteOrigin};
use sn_payments::{
    CheckoutMethod, ClientRef, InvoiceRequest, MethodsContext, PaymentProvider, ProviderKind,
    Unavailable,
};
use sn_store::{Db, InvoiceUpdate, LockSpace, StatusChange, StoreError, Tx};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use uuid::Uuid;

#[cfg(test)]
mod gram_tests;
#[cfg(test)]
mod pool_tests;
#[cfg(test)]
mod promo_tests;
#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;

const POLL_DELAY_SECONDS: i64 = 120;
const CHAIN_POLL_DELAY_SECONDS: i64 = 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CartRequest {
    pub tariff_id: TariffId,
    pub period_id: PeriodId,
    #[serde(default)]
    pub extra_devices: u32,
    #[serde(default)]
    pub subscription_id: Option<SubscriptionId>,
    #[serde(default)]
    pub promo_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Bot,
    Miniapp,
    Cabinet,
    Admin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutRequest {
    pub cart: CartRequest,
    pub provider: String,
    pub channel: Channel,
    #[serde(default)]
    pub return_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuoteView {
    pub quote: Quote,
    pub kind: PlanKind,
    pub expire_preview: DateTime<Utc>,
    pub methods: Vec<CheckoutMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CheckoutResult {
    Redirect {
        order_id: Uuid,
        payment_id: Uuid,
        pay_url: String,
    },
    Completed {
        order_id: Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TariffOffer {
    pub category: Category,
    pub tariff: Tariff,
    pub periods: Vec<PeriodOffer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicTariff {
    pub id: TariffId,
    pub category_id: sn_domain::ids::CategoryId,
    pub name: String,
    pub description: Option<String>,
    pub devices: sn_domain::tariff::DevicePolicy,
    pub traffic: sn_domain::traffic::TrafficPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicOffer {
    pub category: Category,
    pub tariff: PublicTariff,
    pub periods: Vec<PeriodOffer>,
    pub countries: Vec<String>,
}

const COUNTRIES_TTL: std::time::Duration = std::time::Duration::from_secs(600);

type CountryCache = std::sync::Mutex<Option<(std::time::Instant, HashMap<Uuid, Vec<String>>)>>;

static SQUAD_COUNTRIES: CountryCache = std::sync::Mutex::new(None);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRequest {
    pub module: ModuleId,
    pub kind: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub subscription_id: Option<SubscriptionId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemCheckoutRequest {
    pub item: ItemRequest,
    pub provider: String,
    pub channel: Channel,
    #[serde(default)]
    pub return_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemQuoteView {
    pub quote: Quote,
    pub methods: Vec<CheckoutMethod>,
}

struct Sale<'s> {
    order: &'s NewOrder,
    provider: &'s str,
    kind: ProviderKind,
    amount: Amount,
    description: String,
    return_url: Option<String>,
}

struct Basis {
    client: Client,
    tariff: Tariff,
    current: Option<Subscription>,
    kind: PlanKind,
    balance: Money,
}

struct Prepared {
    client: Client,
    tariff: Tariff,
    current: Option<Subscription>,
    kind: PlanKind,
    quote: Quote,
    methods: Vec<CheckoutMethod>,
}

pub(crate) struct PaidOrder {
    pub order: Order,
    pub payment: Payment,
}

struct OrderHooks<'h> {
    modules: &'h ModuleRegistry,
    ctx: &'h ModuleContext,
}

pub(crate) async fn client_ref(db: &mut impl Db, client: &Client) -> ServiceResult<ClientRef> {
    let identities = db.clients().identities(client.id).await?;
    Ok(ClientRef {
        client_id: client.id,
        public_id: client.public_id,
        telegram_id: identities
            .iter()
            .find(|i| i.kind == IdentityKind::Telegram)
            .and_then(|i| i.value.parse().ok()),
        email: identities
            .iter()
            .find(|i| i.kind == IdentityKind::Email)
            .map(|i| i.value.clone()),
    })
}

pub(crate) fn describe(brand: &str, quote: &Quote) -> String {
    match quote.lines.first() {
        Some(line) => format!("{brand}: {}", line.label),
        None => brand.to_string(),
    }
}

pub(crate) async fn record_paid_order(
    tx: &mut Tx,
    order: &NewOrder,
    provider: &str,
    method: Option<&str>,
    amount: Amount,
    debit: Option<LedgerKind>,
) -> ServiceResult<PaidOrder> {
    record_paid(tx, order, provider, method, amount, debit, None).await
}

async fn record_paid(
    tx: &mut Tx,
    order: &NewOrder,
    provider: &str,
    method: Option<&str>,
    amount: Amount,
    debit: Option<LedgerKind>,
    hooks: Option<&OrderHooks<'_>>,
) -> ServiceResult<PaidOrder> {
    if order.kind == OrderKind::TopUp {
        return Err(ServiceError::rejected(
            "checkout.topup_internal",
            "пополнение оплачивается только через платёжную систему",
        ));
    }
    let created = tx.orders().create(order).await?;
    let payment = tx
        .payments()
        .create(&NewPayment {
            order_id: created.id,
            client_id: created.client_id,
            provider: ProviderId::from(provider),
            method: method.map(str::to_string),
            amount,
            expires_at: None,
        })
        .await?;
    if let Some(hooks) = hooks {
        hooks
            .modules
            .order_created(hooks.ctx, tx, &created, &payment)
            .await?;
    }
    if let Some(kind) = debit {
        let money = amount.as_money().ok_or_else(|| {
            ServiceError::rejected(
                "checkout.balance_currency",
                "с баланса оплачивается только сумма в валюте",
            )
        })?;
        let delta = money.checked_neg()?;
        let prefix = match kind {
            LedgerKind::AutoRenew => "autorenew",
            _ => "purchase",
        };
        let appended = tx
            .ledger()
            .append(&NewLedgerEntry {
                client_id: created.client_id,
                wallet: Wallet::Main,
                amount: delta,
                kind,
                order_id: Some(created.id),
                payment_id: Some(payment.id),
                admin_id: None,
                comment: None,
                idempotency_key: Some(format!("{prefix}:order:{}", created.id)),
            })
            .await
            .map_err(|err| match err {
                StoreError::InsufficientFunds => ServiceError::InsufficientFunds,
                other => other.into(),
            })?;
        if !appended.duplicate {
            tx.events()
                .push(&DomainEvent::BalanceChanged {
                    client_id: created.client_id,
                    wallet: Wallet::Main,
                    kind,
                    delta,
                })
                .await?;
        }
    }
    let paid = match tx
        .payments()
        .transition(payment.id, PaymentStatus::Paid, None, None)
        .await?
    {
        StatusChange::Applied(p) | StatusChange::Unchanged(p) => p,
    };
    let (order, _) = tx
        .orders()
        .transition(created.id, OrderStatus::Paid)
        .await?;
    if let Some(hooks) = hooks {
        hooks
            .modules
            .order_paid(hooks.ctx, tx, &order, &paid)
            .await?;
    }
    tx.jobs()
        .enqueue(&JobPayload::FulfillOrder { order_id: order.id }, None)
        .await?;
    tx.events()
        .push(&DomainEvent::PaymentPaid {
            payment_id: paid.id,
            order_id: order.id,
            client_id: order.client_id,
            order_kind: order.kind,
            amount: paid.amount,
            provider: paid.provider.clone(),
        })
        .await?;
    Ok(PaidOrder {
        order,
        payment: paid,
    })
}

fn pick_method(
    methods: &[CheckoutMethod],
    provider: &str,
) -> ServiceResult<(ProviderKind, Amount)> {
    let method = methods
        .iter()
        .find(|m| m.provider == provider)
        .ok_or_else(|| {
            ServiceError::rejected(
                "checkout.method_unavailable",
                "Этот способ оплаты сейчас недоступен",
            )
        })?;
    if let Some(reason) = &method.unavailable {
        return Err(ServiceError::rejected(
            "checkout.method_unavailable",
            match reason {
                Unavailable::InsufficientBalance { balance, .. } => {
                    format!("Не хватает: на балансе {}", format::money(*balance))
                }
                Unavailable::NoStarsPrice => "У этого срока нет цены в звёздах".to_string(),
            },
        ));
    }
    let amount = method.amount.ok_or_else(|| {
        ServiceError::rejected(
            "checkout.method_unavailable",
            "Этот способ оплаты сейчас недоступен",
        )
    })?;
    Ok((method.kind, amount))
}

fn trusted_return_url(url: &str, public_url: Option<&str>) -> bool {
    let same_origin = |base: &str| {
        url == base
            || url
                .strip_prefix(base)
                .is_some_and(|rest| rest.starts_with('/') || rest.starts_with('?'))
    };
    url.starts_with("https://")
        && (public_url.is_some_and(same_origin) || same_origin("https://t.me"))
}

pub(crate) fn trial_snapshot(trial: &TrialSettings) -> TariffSnapshot {
    TariffSnapshot {
        tariff_id: TariffId(0),
        category_id: CategoryId(0),
        name: "Пробный период".into(),
        period_id: PeriodId(0),
        days: trial.days,
        devices_included: trial.devices,
        max_extra_devices: 0,
        traffic: trial.traffic,
        squads: trial.squads.clone(),
        external_squad: None,
    }
}

pub struct Checkout<'a> {
    services: &'a Services,
}

impl<'a> Checkout<'a> {
    pub fn new(services: &'a Services) -> Self {
        Checkout { services }
    }

    pub async fn catalog(&self) -> ServiceResult<Vec<TariffOffer>> {
        self.catalog_in(None).await
    }

    async fn squad_countries(&self, squads: &[Uuid]) -> HashMap<Uuid, Vec<String>> {
        if let Ok(guard) = SQUAD_COUNTRIES.lock()
            && let Some((at, map)) = guard.as_ref()
            && at.elapsed() < COUNTRIES_TTL
            && squads.iter().all(|s| map.contains_key(s))
        {
            return map.clone();
        }
        let mut map = HashMap::new();
        if let Ok(remna) = self.services.remna() {
            for squad in squads {
                let codes = match remna.squad_accessible_nodes(*squad).await {
                    Ok(nodes) => {
                        let mut codes: Vec<String> = Vec::new();
                        for node in nodes.accessible_nodes {
                            let code = node.country_code.to_uppercase();
                            if !code.is_empty() && code != "XX" && !codes.contains(&code) {
                                codes.push(code);
                            }
                        }
                        codes
                    }
                    Err(err) => {
                        tracing::warn!(squad = %squad, error = %err, "страны сквада не получены");
                        Vec::new()
                    }
                };
                map.insert(*squad, codes);
            }
        }
        if let Ok(mut guard) = SQUAD_COUNTRIES.lock() {
            *guard = Some((std::time::Instant::now(), map.clone()));
        }
        map
    }

    pub async fn public_catalog(&self) -> ServiceResult<Vec<PublicOffer>> {
        let offers = self
            .catalog_in(Some(sn_domain::access::ClientChannel::Cabinet))
            .await?;
        let mut squads: Vec<Uuid> = Vec::new();
        for offer in &offers {
            for squad in &offer.tariff.squads {
                if !squads.contains(&squad.0) {
                    squads.push(squad.0);
                }
            }
        }
        let countries = self.squad_countries(&squads).await;
        Ok(offers
            .into_iter()
            .map(|offer| {
                let mut codes: Vec<String> = Vec::new();
                for squad in &offer.tariff.squads {
                    for code in countries.get(&squad.0).into_iter().flatten() {
                        if !codes.contains(code) {
                            codes.push(code.clone());
                        }
                    }
                }
                PublicOffer {
                    category: offer.category,
                    tariff: PublicTariff {
                        id: offer.tariff.id,
                        category_id: offer.tariff.category_id,
                        name: offer.tariff.name,
                        description: offer.tariff.description,
                        devices: offer.tariff.devices,
                        traffic: offer.tariff.traffic,
                    },
                    periods: offer.periods,
                    countries: codes,
                }
            })
            .collect())
    }

    pub async fn catalog_in(
        &self,
        channel: Option<sn_domain::access::ClientChannel>,
    ) -> ServiceResult<Vec<TariffOffer>> {
        let currency = self.services.settings().currency;
        let mut conn = self.services.store().conn().await?;
        let categories = conn.tariffs().categories(false).await?;
        let tariffs = conn.tariffs().list(false).await?;
        let ids: Vec<TariffId> = tariffs.iter().map(|t| t.id).collect();
        let channels = conn.tariffs().channels(&ids).await?;
        let mut offers = Vec::new();
        for category in categories {
            for tariff in tariffs.iter().filter(|t| t.category_id == category.id) {
                if !tariff.is_offered(currency) {
                    continue;
                }
                if let Some(channel) = channel
                    && !channels
                        .get(&tariff.id)
                        .copied()
                        .unwrap_or_default()
                        .allows(channel)
                {
                    continue;
                }
                offers.push(TariffOffer {
                    category: category.clone(),
                    tariff: tariff.clone(),
                    periods: period_offers(tariff, currency),
                });
            }
        }
        Ok(offers)
    }

    async fn basis(
        &self,
        client_id: ClientId,
        cart: &CartRequest,
        currency: Currency,
    ) -> ServiceResult<Basis> {
        let mut conn = self.services.store().conn().await?;
        let client = conn.clients().get(client_id).await?;
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        let tariff = conn.tariffs().get(cart.tariff_id).await?;
        let current = match cart.subscription_id {
            Some(id) => {
                let subscription = conn.subscriptions().get(id).await?;
                if subscription.client_id != client_id {
                    return Err(ServiceError::NotFound("подписка"));
                }
                Some(subscription)
            }
            None => conn
                .subscriptions()
                .by_client(client_id)
                .await?
                .into_iter()
                .find(|s| s.kind == SubscriptionKind::Trial && s.ended_at.is_none()),
        };
        if current
            .as_ref()
            .is_some_and(|s| s.mirror.status == Some(SubscriptionStatus::Disabled))
        {
            return Err(ServiceError::rejected(
                "subscription.disabled",
                "Подписка заблокирована, оплата недоступна. Напишите в поддержку.",
            ));
        }
        let current_category = match current.as_ref().and_then(|s| s.tariff_id) {
            Some(id) => conn.tariffs().get(id).await.ok().map(|t| t.category_id),
            None => None,
        };
        let kind = classify(
            tariff.category_id,
            current.as_ref().map(|s| CurrentTariff {
                kind: s.kind,
                category_id: current_category,
            }),
        );
        let balance = conn
            .ledger()
            .balance(client_id, Wallet::Main, currency)
            .await?;
        Ok(Basis {
            client,
            tariff,
            current,
            kind,
            balance,
        })
    }

    async fn prepare(
        &self,
        client_id: ClientId,
        cart: &CartRequest,
        origin: &QuoteOrigin,
    ) -> ServiceResult<Prepared> {
        let currency = self.services.settings().currency;
        let modules = self.services.modules();
        if cart.promo_code.is_some() && !modules.is_enabled(ModuleId::Promo) {
            return Err(ServiceError::rejected(
                "promo.disabled",
                "Промокоды сейчас не действуют",
            ));
        }
        let Basis {
            client,
            tariff,
            current,
            kind,
            balance,
        } = self.basis(client_id, cart, currency).await?;
        let hook = QuoteHook {
            client_id,
            tariff_id: tariff.id,
            period_id: cart.period_id,
            order_kind: kind.order_kind(),
            currency,
            promo_code: cart.promo_code.clone(),
        };
        let adjustments = modules
            .quote_adjustments_from(&self.services.module_context(), &hook, origin)
            .await?;
        let quote = quote(&QuoteRequest {
            currency,
            tariff: &tariff,
            period_id: cart.period_id,
            extra_devices: cart.extra_devices,
            sell_extra_devices: modules.is_enabled(ModuleId::Options),
            adjustments: &adjustments,
        })?;
        let methods = self.services.payments().checkout_methods(&MethodsContext {
            currency,
            total: quote.total,
            stars_total: quote.stars.map(|s| s.total),
            balance: Some(balance),
        });
        Ok(Prepared {
            client,
            tariff,
            current,
            kind,
            quote,
            methods,
        })
    }

    pub async fn quote(&self, client_id: ClientId, cart: &CartRequest) -> ServiceResult<QuoteView> {
        self.quote_from(client_id, cart, None).await
    }

    pub async fn quote_from(
        &self,
        client_id: ClientId,
        cart: &CartRequest,
        address: Option<IpAddr>,
    ) -> ServiceResult<QuoteView> {
        let prepared = self
            .prepare(client_id, cart, &QuoteOrigin { address })
            .await?;
        let expire_preview = new_expire_at(
            prepared.kind,
            prepared.current.as_ref().and_then(|s| s.mirror.expire_at),
            self.services.now(),
            prepared.quote.days,
        );
        Ok(QuoteView {
            quote: prepared.quote,
            kind: prepared.kind,
            expire_preview,
            methods: prepared.methods,
        })
    }

    pub async fn start(
        &self,
        client_id: ClientId,
        request: &CheckoutRequest,
    ) -> ServiceResult<CheckoutResult> {
        self.start_from(client_id, request, None).await
    }

    pub async fn start_from(
        &self,
        client_id: ClientId,
        request: &CheckoutRequest,
        address: Option<IpAddr>,
    ) -> ServiceResult<CheckoutResult> {
        self.services.remna()?;
        let prepared = self
            .prepare(client_id, &request.cart, &QuoteOrigin { address })
            .await?;
        let (kind, amount) = pick_method(&prepared.methods, &request.provider)?;
        let period = prepared
            .tariff
            .period(request.cart.period_id)
            .ok_or(DomainError::PeriodNotFound(request.cart.period_id))?;
        let target = prepared.current.as_ref().map(|s| s.id);
        let order = NewOrder {
            client_id,
            subscription_id: target,
            kind: prepared.kind.order_kind(),
            intent: PurchaseIntent::Subscription {
                tariff_id: prepared.tariff.id,
                period_id: period.id,
                extra_devices: request.cart.extra_devices,
                target,
            },
            quote: prepared.quote.clone(),
            snapshot: Some(prepared.tariff.snapshot(period)),
        };
        let description = describe(&self.services.settings().brand_name, &prepared.quote);
        self.settle(
            &prepared.client,
            Sale {
                order: &order,
                provider: &request.provider,
                kind,
                amount,
                description,
                return_url: request.return_url.clone(),
            },
        )
        .await
    }

    pub async fn topup(
        &self,
        client_id: ClientId,
        amount: Money,
        provider: &str,
        _channel: Channel,
    ) -> ServiceResult<CheckoutResult> {
        let settings = self.services.settings();
        if amount.currency != settings.currency {
            return Err(ServiceError::rejected(
                "topup.currency",
                format!("Баланс пополняется в {}", settings.currency),
            ));
        }
        let quote = topup_quote(amount, settings.topup_limits())?;
        let client = {
            let mut conn = self.services.store().conn().await?;
            conn.clients().get(client_id).await?
        };
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        let registry = self.services.payments();
        let provider = registry
            .get(provider)
            .filter(|p| {
                p.kind() == ProviderKind::External && registry.is_available(p.id(), amount.currency)
            })
            .ok_or_else(|| {
                ServiceError::rejected(
                    "checkout.method_unavailable",
                    "Пополнить баланс этим способом нельзя",
                )
            })?;
        let order = NewOrder {
            client_id,
            subscription_id: None,
            kind: OrderKind::TopUp,
            intent: PurchaseIntent::TopUp { amount },
            quote: quote.clone(),
            snapshot: None,
        };
        let description = describe(&settings.brand_name, &quote);
        self.invoice(&client, &order, provider, amount.into(), description, None)
            .await
    }

    pub async fn start_trial(&self, client_id: ClientId) -> ServiceResult<OrderId> {
        self.services.remna()?;
        let settings = self.services.settings();
        let trial = &settings.trial;
        if !trial.enabled {
            return Err(ServiceError::rejected(
                "trial.disabled",
                "Пробный период сейчас недоступен",
            ));
        }
        if trial.squads.is_empty() || trial.days == 0 {
            return Err(ServiceError::rejected(
                "trial.not_configured",
                "Пробный период не настроен: укажите срок и сквады",
            ));
        }
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::Client, client_id.0).await?;
        let client = tx.clients().get(client_id).await?;
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        let subscriptions = tx.subscriptions().by_client(client_id).await?;
        if subscriptions.iter().any(|s| s.ended_at.is_none()) {
            return Err(ServiceError::rejected(
                "trial.has_subscription",
                "Пробный период доступен только до первой подписки",
            ));
        }
        let identities: Vec<(IdentityKind, String)> = tx
            .clients()
            .identities(client_id)
            .await?
            .into_iter()
            .map(|i| (i.kind, i.value))
            .collect();
        if identities.is_empty() {
            return Err(ServiceError::rejected(
                "trial.no_identity",
                "Пробный период выдаётся только аккаунту с Telegram или почтой",
            ));
        }
        if !tx.trials().claim(client_id, &identities).await? {
            return Err(ServiceError::rejected(
                "trial.used",
                "Пробный период уже был использован",
            ));
        }
        let currency = settings.currency;
        let order = tx
            .orders()
            .create(&NewOrder {
                client_id,
                subscription_id: None,
                kind: OrderKind::Trial,
                intent: PurchaseIntent::Trial,
                quote: Quote {
                    currency,
                    lines: Vec::new(),
                    subtotal: Money::zero(currency),
                    discount: Money::zero(currency),
                    total: Money::zero(currency),
                    stars: None,
                    days: trial.days,
                    extra_devices: 0,
                },
                snapshot: Some(trial_snapshot(trial)),
            })
            .await?;
        tx.orders().transition(order.id, OrderStatus::Paid).await?;
        tx.jobs()
            .enqueue(&JobPayload::FulfillOrder { order_id: order.id }, None)
            .await?;
        tx.commit().await?;
        Ok(order.id)
    }

    async fn prepare_item(
        &self,
        client_id: ClientId,
        item: &ItemRequest,
    ) -> ServiceResult<(Client, Quote, Vec<CheckoutMethod>)> {
        let currency = self.services.settings().currency;
        let modules = self.services.modules();
        if !modules.is_enabled(item.module) {
            return Err(sn_modules::ModuleError::Disabled(item.module).into());
        }
        let (client, balance) = {
            let mut conn = self.services.store().conn().await?;
            let client = conn.clients().get(client_id).await?;
            if client.is_blocked() {
                return Err(ServiceError::Forbidden);
            }
            if let Some(id) = item.subscription_id {
                let subscription = conn.subscriptions().get(id).await?;
                if subscription.client_id != client_id {
                    return Err(ServiceError::NotFound("подписка"));
                }
            }
            let balance = conn
                .ledger()
                .balance(client_id, Wallet::Main, currency)
                .await?;
            (client, balance)
        };
        let quote = modules
            .get(item.module)
            .quote_item(
                &self.services.module_context(),
                &ItemHook {
                    client_id,
                    currency,
                    kind: item.kind.clone(),
                    payload: item.payload.clone(),
                },
            )
            .await?;
        if quote.currency != currency || quote.total.is_negative() {
            return Err(ServiceError::rejected(
                "checkout.item_price",
                "модуль посчитал цену не в валюте установки",
            ));
        }
        let methods = self.services.payments().checkout_methods(&MethodsContext {
            currency,
            total: quote.total,
            stars_total: quote.stars.map(|s| s.total),
            balance: Some(balance),
        });
        Ok((client, quote, methods))
    }

    pub async fn quote_item(
        &self,
        client_id: ClientId,
        item: &ItemRequest,
    ) -> ServiceResult<ItemQuoteView> {
        let (_, quote, methods) = self.prepare_item(client_id, item).await?;
        Ok(ItemQuoteView { quote, methods })
    }

    pub async fn start_item(
        &self,
        client_id: ClientId,
        request: &ItemCheckoutRequest,
    ) -> ServiceResult<CheckoutResult> {
        let (client, quote, methods) = self.prepare_item(client_id, &request.item).await?;
        let (kind, amount) = pick_method(&methods, &request.provider)?;
        let order = NewOrder {
            client_id,
            subscription_id: request.item.subscription_id,
            kind: OrderKind::Module,
            intent: PurchaseIntent::Module {
                module: request.item.module,
                kind: request.item.kind.clone(),
                payload: request.item.payload.clone(),
            },
            quote: quote.clone(),
            snapshot: None,
        };
        let description = describe(&self.services.settings().brand_name, &quote);
        self.settle(
            &client,
            Sale {
                order: &order,
                provider: &request.provider,
                kind,
                amount,
                description,
                return_url: request.return_url.clone(),
            },
        )
        .await
    }

    async fn settle(&self, client: &Client, sale: Sale<'_>) -> ServiceResult<CheckoutResult> {
        let ctx = self.services.module_context();
        let hooks = OrderHooks {
            modules: self.services.modules(),
            ctx: &ctx,
        };
        if sale.amount.minor == 0 {
            let mut tx = self.services.store().begin().await?;
            let paid = record_paid(
                &mut tx,
                sale.order,
                providers::MANUAL,
                Some("free"),
                sale.amount,
                None,
                Some(&hooks),
            )
            .await?;
            tx.commit().await?;
            return Ok(CheckoutResult::Completed {
                order_id: paid.order.public_id,
            });
        }
        match sale.kind {
            ProviderKind::Balance => {
                let mut tx = self.services.store().begin().await?;
                let paid = record_paid(
                    &mut tx,
                    sale.order,
                    providers::BALANCE,
                    None,
                    sale.amount,
                    Some(LedgerKind::Purchase),
                    Some(&hooks),
                )
                .await?;
                tx.commit().await?;
                Ok(CheckoutResult::Completed {
                    order_id: paid.order.public_id,
                })
            }
            ProviderKind::External | ProviderKind::Stars => {
                let provider = self
                    .services
                    .payments()
                    .get(sale.provider)
                    .ok_or(ServiceError::NotFound("платёжная система"))?;
                self.invoice(
                    client,
                    sale.order,
                    provider,
                    sale.amount,
                    sale.description,
                    sale.return_url,
                )
                .await
            }
        }
    }

    async fn invoice(
        &self,
        client: &Client,
        order: &NewOrder,
        provider: Arc<dyn PaymentProvider>,
        amount: Amount,
        description: String,
        return_url: Option<String>,
    ) -> ServiceResult<CheckoutResult> {
        let settings = self.services.settings();
        let public_url = settings
            .public_url
            .as_deref()
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty());
        let external = provider.kind() == ProviderKind::External;
        if external && public_url.is_none() && !provider.polls_status() {
            return Err(ServiceError::rejected(
                "config.public_url",
                "Не задан внешний адрес панели: платёжная система не сможет прислать уведомление",
            ));
        }
        if amount.currency == PayCurrency::Xtr && provider.kind() != ProviderKind::Stars {
            return Err(ServiceError::rejected(
                "checkout.method_unavailable",
                "Звёздами оплачивается только счёт в Telegram",
            ));
        }
        let now = self.services.now();
        let ttl = Duration::minutes(settings.invoice_ttl_minutes as i64);
        let ctx = self.services.module_context();
        let modules = self.services.modules();
        let mut tx = self.services.store().begin().await?;
        let created = tx.orders().create(order).await?;
        let payment = tx
            .payments()
            .create(&NewPayment {
                order_id: created.id,
                client_id: created.client_id,
                provider: ProviderId::from(provider.id()),
                method: None,
                amount,
                expires_at: Some(now + ttl),
            })
            .await?;
        modules
            .order_created(&ctx, &mut tx, &created, &payment)
            .await?;
        if external && (provider.accepts_http_webhooks() || provider.polls_status()) {
            let delay = if provider.polls_status() {
                CHAIN_POLL_DELAY_SECONDS
            } else {
                POLL_DELAY_SECONDS
            };
            tx.jobs()
                .enqueue(
                    &JobPayload::PollPayment {
                        payment_id: payment.id,
                    },
                    Some(now + Duration::seconds(delay)),
                )
                .await?;
        }
        let client_ref = client_ref(&mut tx, client).await?;
        tx.commit().await?;
        let request = InvoiceRequest {
            payment_id: payment.id,
            payment_public_id: payment.public_id,
            order_public_id: created.public_id,
            amount,
            description,
            client: client_ref,
            return_url: return_url.filter(|u| trusted_return_url(u, public_url.as_deref())),
            webhook_url: public_url
                .as_deref()
                .map(|base| format!("{base}/api/webhooks/pay/{}", provider.id()))
                .unwrap_or_default(),
            expires_in: ttl,
            save_payment_method: provider.supports_recurring()
                && settings.autorenew.use_saved_card
                && matches!(
                    created.kind,
                    OrderKind::New | OrderKind::Renewal | OrderKind::Change
                ),
        };
        let config = self.services.payments().config(provider.id());
        match provider.create_invoice(&config, &request).await {
            Ok(invoice) => {
                let mut conn = self.services.store().conn().await?;
                conn.payments()
                    .attach_invoice(
                        payment.id,
                        &InvoiceUpdate {
                            pay_url: Some(invoice.pay_url.clone()),
                            provider_txid: invoice.external_id.clone(),
                            expires_at: invoice.expires_at,
                            payload: invoice.payload.clone(),
                        },
                    )
                    .await?;
                let pay_url = match &public_url {
                    Some(base) => format!("{base}/api/pay/{}", payment.public_id),
                    None => invoice.pay_url,
                };
                Ok(CheckoutResult::Redirect {
                    order_id: created.public_id,
                    payment_id: payment.public_id,
                    pay_url,
                })
            }
            Err(err) => {
                tracing::warn!(payment = %payment.id, provider = provider.id(), error = %err, "счёт не выставлен");
                let reason = err.to_string();
                let mut tx = self.services.store().begin().await?;
                match tx
                    .payments()
                    .transition(payment.id, PaymentStatus::Failed, None, Some(&reason))
                    .await
                {
                    Ok(_) | Err(StoreError::Invalid(_)) => {}
                    Err(other) => return Err(other.into()),
                }
                match tx
                    .orders()
                    .transition(created.id, OrderStatus::Canceled)
                    .await
                {
                    Ok((canceled, true)) => {
                        modules.order_canceled(&ctx, &mut tx, &canceled).await?;
                    }
                    Ok(_) | Err(StoreError::Invalid(_)) => {}
                    Err(other) => return Err(other.into()),
                }
                tx.commit().await?;
                Err(err.into())
            }
        }
    }
}
