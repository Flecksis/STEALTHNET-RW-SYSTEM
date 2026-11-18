use crate::Actor;
use crate::checkout::single_subscription;
use crate::error::ServiceResult;
use crate::services::Services;
use chrono::{DateTime, Utc};
use sn_domain::ids::{CategoryId, PeriodId, TariffId};
use sn_domain::jobs::JobPayload;
use sn_domain::money::Money;
use sn_domain::order::{NewOrder, OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::plan::{CurrentTariff, PlanKind, classify};
use sn_domain::pricing::{Quote, QuoteLine, QuoteLineKind};
use sn_domain::subscription::{Subscription, SubscriptionKind};
use sn_domain::tariff::{TariffSnapshot, months_for_days};
use sn_modules::{
    GrantCurrent, GrantPreview, GrantRefusal, GrantSpec, GrantTarget, GrantedOrder,
    SubscriptionGrant,
};
use sn_store::{Db, LockSpace, StoreError, Tx};

#[cfg(test)]
mod tests;

pub const GRANT_AUDIT: &str = "subscription.module_grant";

struct Decision {
    snapshot: TariffSnapshot,
    plan: PlanKind,
    target: Option<Subscription>,
    current: Option<GrantCurrent>,
    choices: Vec<GrantCurrent>,
    multi: bool,
}

fn refusal(code: &'static str, message: &str) -> GrantRefusal {
    let english = match code {
        "grant.tariff_missing" => {
            "The plan of this subscription no longer exists. Please contact support."
        }
        "grant.no_squads" => "This subscription has no servers. Please contact support.",
        "grant.client_blocked" => "Your account is blocked. Please contact support.",
        "grant.target_missing" => "This subscription wasn't found or has already ended.",
        "grant.has_subscription" => {
            "You already have an active subscription, and this gives a new one. You can use it once your subscription ends."
        }
        "grant.unavailable" => "Subscriptions can't be issued right now. Please try later.",
        _ => message,
    };
    GrantRefusal::new(code, sn_domain::lang::pick(message, english))
}

fn active(subscription: &Subscription, now: DateTime<Utc>) -> bool {
    subscription.remna.is_some() && subscription.mirror.expire_at.is_some_and(|at| at > now)
}

async fn category_of(
    db: &mut impl Db,
    subscription: &Subscription,
) -> ServiceResult<Option<CategoryId>> {
    let Some(id) = subscription.tariff_id else {
        return Ok(None);
    };
    match db.tariffs().get(id).await {
        Ok(tariff) => Ok(Some(tariff.category_id)),
        Err(StoreError::NotFound(_)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

async fn describe(
    db: &mut impl Db,
    subscription: &Subscription,
    now: DateTime<Utc>,
) -> ServiceResult<GrantCurrent> {
    let tariff = match subscription.tariff_id {
        Some(id) => match db.tariffs().get(id).await {
            Ok(tariff) => Some(tariff.name),
            Err(StoreError::NotFound(_)) => None,
            Err(err) => return Err(err.into()),
        },
        None => None,
    };
    Ok(GrantCurrent {
        subscription_id: subscription.id,
        tariff,
        kind: subscription.kind,
        expire_at: subscription.mirror.expire_at,
        active: active(subscription, now),
    })
}

pub fn custom_snapshot(name: &str, days: u32, spec: &GrantSpec) -> Option<TariffSnapshot> {
    let GrantSpec::Custom {
        devices,
        traffic,
        squads,
        ..
    } = spec
    else {
        return None;
    };
    Some(TariffSnapshot {
        traffic_leftover: Default::default(),
        tariff_id: TariffId(0),
        category_id: CategoryId(0),
        name: name.to_string(),
        period_id: PeriodId(0),
        days,
        devices_included: *devices,
        max_extra_devices: 0,
        traffic: *traffic,
        squads: squads.clone(),
        external_squad: None,
    })
}

async fn snapshot(
    db: &mut impl Db,
    spec: &GrantSpec,
) -> ServiceResult<Result<TariffSnapshot, GrantRefusal>> {
    match spec {
        GrantSpec::Tariff {
            tariff_id,
            period_id,
            days,
        } => {
            let tariff = match db.tariffs().get(*tariff_id).await {
                Ok(tariff) => tariff,
                Err(StoreError::NotFound(_)) => {
                    return Ok(Err(refusal(
                        "grant.tariff_missing",
                        "Тарифа этой подписки больше нет. Напишите в поддержку.",
                    )));
                }
                Err(err) => return Err(err.into()),
            };
            if tariff.squads.is_empty() {
                return Ok(Err(refusal(
                    "grant.no_squads",
                    "У тарифа этой подписки нет серверов. Напишите в поддержку.",
                )));
            }
            let Some(period) = tariff
                .period(*period_id)
                .or_else(|| tariff.periods.iter().find(|p| p.active))
                .or_else(|| tariff.periods.first())
            else {
                return Ok(Err(refusal(
                    "grant.tariff_missing",
                    "Тарифа этой подписки больше нет. Напишите в поддержку.",
                )));
            };
            let mut snapshot = tariff.snapshot(period);
            snapshot.days = *days;
            Ok(Ok(snapshot))
        }
        GrantSpec::Custom {
            name, days, squads, ..
        } => {
            if squads.is_empty() {
                return Ok(Err(refusal(
                    "grant.no_squads",
                    "У этой подписки нет серверов. Напишите в поддержку.",
                )));
            }
            Ok(custom_snapshot(name, *days, spec)
                .ok_or_else(|| refusal("grant.no_squads", "У этой подписки нет серверов.")))
        }
    }
}

async fn decide(
    services: &Services,
    db: &mut impl Db,
    grant: &SubscriptionGrant,
) -> ServiceResult<Result<Decision, GrantRefusal>> {
    let client = db.clients().get(grant.client_id).await?;
    if client.is_blocked() {
        return Ok(Err(refusal(
            "grant.client_blocked",
            "Аккаунт заблокирован. Напишите в поддержку.",
        )));
    }
    let snapshot = match snapshot(db, &grant.spec).await? {
        Ok(snapshot) => snapshot,
        Err(refused) => return Ok(Err(refused)),
    };
    let custom = matches!(grant.spec, GrantSpec::Custom { .. });
    let now = services.now();
    let multi = services.settings().multi_subscriptions;
    let live: Vec<Subscription> = db
        .subscriptions()
        .by_client(grant.client_id)
        .await?
        .into_iter()
        .filter(|s| s.ended_at.is_none())
        .collect();
    let target: Option<(Subscription, Option<CategoryId>)> = if !multi {
        single_subscription(db, grant.client_id, snapshot.category_id).await?
    } else {
        let picked = match grant.target {
            GrantTarget::New => None,
            GrantTarget::Subscription(id) => match live.iter().find(|s| s.id == id) {
                Some(found) => Some(found.clone()),
                None => {
                    return Ok(Err(refusal(
                        "grant.target_missing",
                        "Эта подписка не найдена или уже завершена.",
                    )));
                }
            },
            GrantTarget::Auto if custom => None,
            GrantTarget::Auto => live
                .iter()
                .filter(|s| s.kind == SubscriptionKind::Trial)
                .max_by_key(|s| s.id.0)
                .cloned(),
        };
        match picked {
            Some(subscription) => {
                let category = category_of(db, &subscription).await?;
                Some((subscription, category))
            }
            None => None,
        }
    };
    if custom
        && let Some((current, _)) = &target
        && current.kind != SubscriptionKind::Trial
        && active(current, now)
    {
        return Ok(Err(refusal(
            "grant.has_subscription",
            "У вас уже есть действующая подписка, а эта выдаёт новую. Воспользоваться можно будет, когда подписка закончится.",
        )));
    }
    let plan = classify(
        snapshot.category_id,
        target.as_ref().map(|(s, category)| CurrentTariff {
            kind: s.kind,
            category_id: *category,
        }),
    );
    let current = match &target {
        Some((subscription, _)) => Some(describe(db, subscription, now).await?),
        None => None,
    };
    let mut choices = Vec::new();
    if multi && !custom {
        for subscription in live.iter().filter(|s| s.kind != SubscriptionKind::Trial) {
            if category_of(db, subscription).await? == Some(snapshot.category_id) {
                choices.push(describe(db, subscription, now).await?);
            }
        }
    }
    Ok(Ok(Decision {
        snapshot,
        plan,
        target: target.map(|(s, _)| s),
        current,
        choices,
        multi,
    }))
}

fn grant_quote(services: &Services, grant: &SubscriptionGrant, snapshot: &TariffSnapshot) -> Quote {
    let currency = services.settings().currency;
    let days = snapshot.days;
    Quote {
        currency,
        lines: vec![QuoteLine {
            kind: QuoteLineKind::Period {
                tariff_id: snapshot.tariff_id,
                period_id: snapshot.period_id,
                days,
                months: months_for_days(days),
            },
            label: grant.label.clone(),
            amount: Money::zero(currency),
            stars: None,
        }],
        subtotal: Money::zero(currency),
        discount: Money::zero(currency),
        total: Money::zero(currency),
        stars: None,
        days,
        extra_devices: 0,
    }
}

pub struct Grants<'a> {
    services: &'a Services,
}

impl<'a> Grants<'a> {
    pub fn new(services: &'a Services) -> Self {
        Grants { services }
    }

    pub async fn preview(&self, grant: &SubscriptionGrant) -> ServiceResult<GrantPreview> {
        let mut conn = self.services.store().conn().await?;
        let decision = decide(self.services, &mut conn, grant).await?;
        Ok(match decision {
            Ok(decision) => GrantPreview {
                plan: decision.plan,
                multi: decision.multi,
                current: decision.current,
                choices: decision.choices,
                refusal: None,
            },
            Err(refused) => GrantPreview {
                plan: PlanKind::New,
                multi: self.services.settings().multi_subscriptions,
                current: None,
                choices: Vec::new(),
                refusal: Some(refused),
            },
        })
    }

    pub async fn grant(
        &self,
        tx: &mut Tx,
        grant: &SubscriptionGrant,
    ) -> ServiceResult<Result<GrantedOrder, GrantRefusal>> {
        if self.services.remna().is_err() {
            return Ok(Err(refusal(
                "grant.unavailable",
                "Выдача подписок сейчас недоступна. Попробуйте позже.",
            )));
        }
        if grant.spec.days() == 0 {
            return Ok(Err(refusal("grant.days", "У подписки не задан срок.")));
        }
        tx.lock(LockSpace::Client, grant.client_id.0).await?;
        let decision = match decide(self.services, tx, grant).await? {
            Ok(decision) => decision,
            Err(refused) => return Ok(Err(refused)),
        };
        let (tariff_id, period_id) = match grant.spec {
            GrantSpec::Tariff {
                tariff_id,
                period_id,
                ..
            } => (Some(tariff_id), Some(period_id)),
            GrantSpec::Custom { .. } => (None, None),
        };
        let quote = grant_quote(self.services, grant, &decision.snapshot);
        let order = tx
            .orders()
            .create(&NewOrder {
                client_id: grant.client_id,
                subscription_id: decision.target.as_ref().map(|s| s.id),
                kind: OrderKind::ModuleGrant,
                intent: PurchaseIntent::ModuleGrant {
                    module: grant.module,
                    reference: grant.reference.clone(),
                    tariff_id,
                    period_id,
                    days: grant.spec.days(),
                },
                quote,
                snapshot: Some(decision.snapshot.clone()),
            })
            .await?;
        tx.orders().transition(order.id, OrderStatus::Paid).await?;
        tx.jobs()
            .enqueue(&JobPayload::FulfillOrder { order_id: order.id }, None)
            .await?;
        tx.audit()
            .record(&Actor::System.audit(
                GRANT_AUDIT,
                "client",
                Some(grant.client_id.0),
                serde_json::json!({
                    "module": grant.module,
                    "reference": grant.reference,
                    "order_id": order.public_id,
                    "label": grant.label,
                    "days": grant.spec.days(),
                    "plan": decision.plan,
                    "subscription_id": decision.target.as_ref().map(|s| s.id),
                }),
            ))
            .await?;
        Ok(Ok(GrantedOrder {
            order_id: order.id,
            order_public_id: order.public_id,
            plan: decision.plan,
            subscription_id: decision.target.map(|s| s.id),
        }))
    }
}
