use crate::error::ServiceResult;
use crate::services::Services;
use chrono::{DateTime, Duration, Utc};
use sn_domain::ids::{ClientId, EventId};
use sn_domain::module::ModuleId;
use sn_modules::ModuleContext;
use sn_store::{ActorKind, AuditEntry, Db, Delivery, DeliveryStatus, StoredEvent, Tx};
use std::collections::{HashMap, HashSet};

#[cfg(test)]
mod tests;

pub const MAX_DELIVERY_ATTEMPTS: u32 = 12;
const FIRST_RETRY_SECONDS: i64 = 5;
const LONGEST_RETRY_SECONDS: i64 = 3_600;
const ERROR_LENGTH: usize = 500;

pub fn retry_delay(attempts: u32) -> Duration {
    let shift = attempts.saturating_sub(1).min(20);
    Duration::seconds((FIRST_RETRY_SECONDS << shift).min(LONGEST_RETRY_SECONDS))
}

fn earliest(current: Option<DateTime<Utc>>, next: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (current, next) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

struct Round {
    ctx: ModuleContext,
    now: DateTime<Utc>,
    waiting: HashSet<(ModuleId, ClientId)>,
}

pub struct EventDispatcher<'a> {
    services: &'a Services,
}

impl<'a> EventDispatcher<'a> {
    pub fn new(services: &'a Services) -> Self {
        EventDispatcher { services }
    }

    pub async fn dispatch_pending(&self, limit: i64) -> ServiceResult<usize> {
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        if !tx.events().try_lock_dispatch().await? {
            tx.rollback().await?;
            return Ok(0);
        }
        let events = tx.events().claim_due(now, limit).await?;
        if events.is_empty() {
            tx.commit().await?;
            return Ok(0);
        }
        let ids: Vec<EventId> = events.iter().map(|e| e.id).collect();
        let mut known: HashMap<EventId, Vec<Delivery>> = HashMap::new();
        for delivery in tx.events().deliveries(&ids).await? {
            known.entry(delivery.event_id).or_default().push(delivery);
        }
        let mut round = Round {
            ctx: self.services.module_context(),
            now,
            waiting: tx.events().waiting_modules().await?.into_iter().collect(),
        };
        let mut finished = Vec::new();
        for stored in &events {
            let (fresh, targets) = match known.remove(&stored.id) {
                None => (
                    true,
                    self.services
                        .modules()
                        .recipients()
                        .into_iter()
                        .map(|module| Delivery::new(stored.id, module))
                        .collect::<Vec<_>>(),
                ),
                Some(rows) => (
                    false,
                    rows.into_iter()
                        .filter(|d| d.status == DeliveryStatus::Pending)
                        .collect(),
                ),
            };
            let mut next = None;
            for delivery in targets {
                let delivery = self.attempt(&mut tx, &round, stored, delivery).await?;
                if delivery.status == DeliveryStatus::Pending {
                    round
                        .waiting
                        .insert((delivery.module, stored.event.client_id()));
                    next = earliest(next, delivery.next_attempt_at);
                }
                if !(fresh && delivery.status == DeliveryStatus::Done) {
                    tx.events().save_delivery(&delivery).await?;
                }
            }
            match next {
                None => finished.push(stored.id),
                Some(at) => tx.events().defer(stored.id, at).await?,
            }
        }
        tx.events().mark_dispatched(&finished).await?;
        tx.commit().await?;
        Ok(events.len())
    }

    async fn attempt(
        &self,
        tx: &mut Tx,
        round: &Round,
        stored: &StoredEvent,
        mut delivery: Delivery,
    ) -> ServiceResult<Delivery> {
        let now = round.now;
        if delivery.next_attempt_at.is_some_and(|at| at > now) {
            return Ok(delivery);
        }
        let modules = self.services.modules();
        if !modules.is_enabled(delivery.module) {
            delivery.status = DeliveryStatus::Skipped;
            delivery.next_attempt_at = None;
            return Ok(delivery);
        }
        let client_id = stored.event.client_id();
        if round.waiting.contains(&(delivery.module, client_id))
            && let Some(at) = tx
                .events()
                .blocked_until(delivery.module, client_id, stored.id)
                .await?
        {
            delivery.status = DeliveryStatus::Pending;
            delivery.next_attempt_at = Some(at.max(now));
            return Ok(delivery);
        }
        match modules
            .deliver(&round.ctx, delivery.module, &stored.event)
            .await
        {
            Ok(()) => {
                delivery.status = DeliveryStatus::Done;
                delivery.next_attempt_at = None;
            }
            Err(err) => {
                delivery.attempts = delivery.attempts.saturating_add(1);
                delivery.last_error = Some(err.to_string().chars().take(ERROR_LENGTH).collect());
                if delivery.attempts >= MAX_DELIVERY_ATTEMPTS {
                    delivery.status = DeliveryStatus::Review;
                    delivery.next_attempt_at = None;
                    tracing::error!(
                        module = delivery.module.as_str(),
                        event = stored.event.kind(),
                        event_id = stored.id.0,
                        attempts = delivery.attempts,
                        error = %err,
                        "модуль так и не обработал событие, нужна ручная проверка"
                    );
                    tx.audit()
                        .record(&AuditEntry {
                            actor_kind: ActorKind::System,
                            actor_id: None,
                            action: "module.event_review".into(),
                            entity_kind: Some("domain_event".into()),
                            entity_id: Some(stored.id.0),
                            details: serde_json::json!({
                                "module": delivery.module.as_str(),
                                "event": stored.event.kind(),
                                "attempts": delivery.attempts,
                                "error": delivery.last_error,
                            }),
                            ip: None,
                        })
                        .await?;
                } else {
                    let at = now + retry_delay(delivery.attempts);
                    delivery.status = DeliveryStatus::Pending;
                    delivery.next_attempt_at = Some(at);
                    tracing::warn!(
                        module = delivery.module.as_str(),
                        event = stored.event.kind(),
                        event_id = stored.id.0,
                        attempts = delivery.attempts,
                        retry_at = %at,
                        error = %err,
                        "модуль не обработал событие, повторим"
                    );
                }
            }
        }
        Ok(delivery)
    }

    pub async fn review(&self, limit: i64) -> ServiceResult<Vec<Delivery>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.events().in_review(limit).await?)
    }

    pub async fn retry(&self, event_id: EventId, module: ModuleId) -> ServiceResult<bool> {
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let changed = tx.events().retry_delivery(event_id, module, now).await?;
        tx.commit().await?;
        Ok(changed)
    }
}
