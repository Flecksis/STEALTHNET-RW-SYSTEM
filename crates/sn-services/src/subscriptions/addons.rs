use super::{
    SubscriptionOps, actions, actor_fields, confirm, ensure_live, linked_user,
    with_subscription_lock,
};
use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::remna_sync::store_user;
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sn_domain::ids::{OrderId, SubscriptionId};
use sn_domain::subscription::{ExtraTraffic, Subscription};
use sn_domain::tariff::TrafficLeftover;
use sn_domain::traffic::{NextReset, TrafficReset, TrafficStrategy, next_reset, unused_extra};
use sn_remnawave::{RemnaUser, UpdateUserRequest, UserStatus};
use sn_store::{AuditEntry, Db, Tx};

const SETTLE_BATCH: i64 = 100;
const RESET_GRACE_HOURS: i64 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hold {
    held_limit: u32,
    limit_after: u32,
}

fn bytes_of(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn window_mode(strategy: TrafficStrategy) -> Option<TrafficReset> {
    match strategy {
        TrafficStrategy::NoReset => None,
        TrafficStrategy::Day => Some(TrafficReset::Day),
        TrafficStrategy::Week => Some(TrafficReset::Week),
        TrafficStrategy::Month => Some(TrafficReset::Month),
        TrafficStrategy::MonthRolling => Some(TrafficReset::MonthRolling),
    }
}

fn lifetime_at_reset(user: &RemnaUser) -> u64 {
    bytes_of(user.user_traffic.lifetime_used_traffic_bytes)
        .saturating_sub(bytes_of(user.user_traffic.used_traffic_bytes))
}

fn traffic_left(limit: u64, user: &RemnaUser) -> bool {
    limit == 0 || limit > bytes_of(user.user_traffic.used_traffic_bytes)
}

async fn audit(
    tx: &mut Tx,
    subscription: &Subscription,
    action: &str,
    mut details: serde_json::Value,
    now: DateTime<Utc>,
) -> ServiceResult<()> {
    if let Some(object) = details.as_object_mut() {
        object.insert("client_id".into(), json!(subscription.client_id));
        object.insert("at".into(), json!(now));
    }
    let (actor_kind, actor_id) = actor_fields(Actor::System);
    tx.audit()
        .record(&AuditEntry {
            actor_kind,
            actor_id,
            action: action.to_string(),
            entity_kind: Some("subscription".into()),
            entity_id: Some(subscription.id.0),
            details,
            ip: None,
        })
        .await?;
    Ok(())
}

impl SubscriptionOps<'_> {
    fn window_for(&self, user: &RemnaUser, bytes: u64, now: DateTime<Utc>) -> ExtraTraffic {
        let ends = window_mode(user.traffic_limit_strategy).and_then(|mode| {
            match next_reset(
                mode,
                user.created_at,
                now,
                self.services.settings().remna_offset(),
            ) {
                NextReset::At(at) => Some(at),
                NextReset::OnPayment | NextReset::Never => None,
            }
        });
        ExtraTraffic {
            subscription_id: SubscriptionId(0),
            bytes,
            window_started_at: ends.map(|_| user.last_traffic_reset_at.unwrap_or(now)),
            window_lifetime: ends.map(|_| lifetime_at_reset(user)),
            window_ends_at: ends,
        }
    }

    async fn first_in_line(&self, id: SubscriptionId, order_id: OrderId) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        match conn.orders().delivery_blocker(id, order_id).await? {
            Some(public_id) => Err(ServiceError::rejected(
                "addon.wait",
                format!("сначала выдаётся заказ {public_id}"),
            )),
            None => Ok(()),
        }
    }

    async fn active_hold(
        &self,
        id: SubscriptionId,
        now: DateTime<Utc>,
    ) -> ServiceResult<Option<Hold>> {
        let mut conn = self.services.store().conn().await?;
        let row: Option<(i32, i32)> = sqlx::query_as(
            "SELECT held_limit, limit_after FROM subscription_device_holds \
             WHERE subscription_id = $1 AND held_until > $2",
        )
        .bind(id.0)
        .bind(now)
        .fetch_optional(conn.conn())
        .await?;
        Ok(row.map(|(held, after)| Hold {
            held_limit: u32::try_from(held).unwrap_or(0),
            limit_after: u32::try_from(after).unwrap_or(0),
        }))
    }

    pub async fn apply_device_addon(
        &self,
        order_id: OrderId,
        id: SubscriptionId,
        devices_after: u32,
    ) -> ServiceResult<()> {
        let subscription = self.owned(id, Actor::System).await?;
        ensure_live(&subscription)?;
        self.first_in_line(id, order_id).await?;
        let user_id = linked_user(&subscription)?;
        let tariff_id = subscription
            .tariff_id
            .ok_or_else(|| ServiceError::rejected("addon.no_tariff", "у подписки нет тарифа"))?;
        let included = {
            let mut conn = self.services.store().conn().await?;
            conn.tariffs().get(tariff_id).await?.devices.included
        };
        let remna = self.services.remna()?;
        with_subscription_lock(self.services, id, async {
            let now = self.services.now();
            let subscription = self.owned(id, Actor::System).await?;
            let extra = subscription.extra_devices.max(devices_after);
            let paid = included.saturating_add(extra);
            let hold = self.active_hold(id, now).await?;
            let target = hold.map_or(paid, |h| h.held_limit.max(paid));
            let before = self.fetch_user(&*remna, &subscription, user_id).await?;
            let after = if before.hwid_device_limit == Some(i64::from(target)) {
                before.clone()
            } else {
                let mut request = UpdateUserRequest::new(user_id);
                request.hwid_device_limit = Some(Some(i64::from(target)));
                match remna.update_user(&request).await {
                    Ok(user) => user,
                    Err(err) => {
                        confirm(&*remna, user_id, err, |u| {
                            u.hwid_device_limit == Some(i64::from(target))
                        })
                        .await?
                    }
                }
            };
            let mut tx = self.services.store().begin().await?;
            tx.subscriptions().set_extra_devices(id, extra).await?;
            if let Some(hold) = hold
                && paid > hold.limit_after
            {
                sqlx::query(
                    "UPDATE subscription_device_holds SET limit_after = $2 WHERE subscription_id = $1",
                )
                .bind(id.0)
                .bind(i32::try_from(paid).unwrap_or(i32::MAX))
                .execute(tx.conn())
                .await?;
            }
            store_user(&mut tx, id, &after, None, now).await?;
            audit(
                &mut tx,
                &subscription,
                actions::DEVICE_ADDON,
                json!({
                    "order_id": order_id,
                    "extra_devices_before": subscription.extra_devices,
                    "extra_devices": extra,
                    "hwid_limit_before": before.hwid_device_limit,
                    "hwid_limit": after.hwid_device_limit,
                }),
                now,
            )
            .await?;
            tx.commit().await?;
            Ok(())
        })
        .await
    }

    pub async fn apply_traffic_addon(
        &self,
        order_id: OrderId,
        id: SubscriptionId,
        bytes: u64,
    ) -> ServiceResult<()> {
        let subscription = self.owned(id, Actor::System).await?;
        ensure_live(&subscription)?;
        self.first_in_line(id, order_id).await?;
        let user_id = linked_user(&subscription)?;
        let remna = self.services.remna()?;
        with_subscription_lock(self.services, id, async {
            let now = self.services.now();
            let marker: Option<(i64, Option<DateTime<Utc>>)> = {
                let mut conn = self.services.store().conn().await?;
                sqlx::query_as(
                    "SELECT limit_target, applied_at FROM extra_traffic_orders WHERE order_id = $1",
                )
                .bind(order_id.0)
                .fetch_optional(conn.conn())
                .await?
            };
            if matches!(marker, Some((_, Some(_)))) {
                return Ok(());
            }
            let before = self.fetch_user(&*remna, &subscription, user_id).await?;
            let limit_before = bytes_of(before.traffic_limit_bytes);
            if limit_before == 0 {
                return Err(ServiceError::rejected(
                    "addon.unlimited",
                    "у подписки нет лимита трафика, докупать нечего",
                ));
            }
            let target = match marker {
                Some((target, _)) => bytes_of(target),
                None => {
                    let target = limit_before.saturating_add(bytes);
                    let mut conn = self.services.store().conn().await?;
                    sqlx::query(
                        "INSERT INTO extra_traffic_orders (order_id, subscription_id, bytes, limit_target) \
                         VALUES ($1, $2, $3, $4) ON CONFLICT (order_id) DO NOTHING",
                    )
                    .bind(order_id.0)
                    .bind(id.0)
                    .bind(i64::try_from(bytes).unwrap_or(i64::MAX))
                    .bind(i64::try_from(target).unwrap_or(i64::MAX))
                    .execute(conn.conn())
                    .await?;
                    target
                }
            };
            let wake = before.status == UserStatus::Limited && traffic_left(target, &before);
            let after = if limit_before == target && !wake {
                before.clone()
            } else {
                let mut request = UpdateUserRequest::new(user_id);
                request.traffic_limit_bytes = Some(i64::try_from(target).unwrap_or(i64::MAX));
                if wake {
                    request.status = Some(UserStatus::Active);
                }
                match remna.update_user(&request).await {
                    Ok(user) => user,
                    Err(err) => {
                        confirm(&*remna, user_id, err, |u| bytes_of(u.traffic_limit_bytes) == target)
                            .await?
                    }
                }
            };
            let mut tx = self.services.store().begin().await?;
            let current = tx.subscriptions().extra_traffic(id).await?;
            let state = match current {
                Some(mut state) => {
                    state.bytes = state.bytes.saturating_add(bytes);
                    state
                }
                None => ExtraTraffic {
                    subscription_id: id,
                    ..self.window_for(&after, bytes, now)
                },
            };
            tx.subscriptions().set_extra_traffic(&state).await?;
            sqlx::query("UPDATE extra_traffic_orders SET applied_at = $2 WHERE order_id = $1")
                .bind(order_id.0)
                .bind(now)
                .execute(tx.conn())
                .await?;
            store_user(&mut tx, id, &after, None, now).await?;
            audit(
                &mut tx,
                &subscription,
                actions::TRAFFIC_ADDON,
                json!({
                    "order_id": order_id,
                    "bytes": bytes,
                    "traffic_limit_before": limit_before,
                    "traffic_limit": bytes_of(after.traffic_limit_bytes),
                    "extra_traffic": state.bytes,
                }),
                now,
            )
            .await?;
            tx.commit().await?;
            Ok(())
        })
        .await
    }

    pub async fn extra_traffic(&self, id: SubscriptionId) -> ServiceResult<Option<ExtraTraffic>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.subscriptions().extra_traffic(id).await?)
    }

    pub async fn settle_extra_traffic(&self, now: DateTime<Utc>) -> ServiceResult<usize> {
        let due = {
            let mut conn = self.services.store().conn().await?;
            conn.subscriptions()
                .extra_traffic_due(now, SETTLE_BATCH)
                .await?
        };
        let mut settled = 0;
        for id in due {
            match self.settle_window(id, now).await {
                Ok(true) => settled += 1,
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(subscription = %id, error = %err, "докупленный трафик не сверен");
                }
            }
        }
        Ok(settled)
    }

    async fn settle_window(&self, id: SubscriptionId, now: DateTime<Utc>) -> ServiceResult<bool> {
        let remna = self.services.remna()?;
        with_subscription_lock(self.services, id, async {
            let mut conn = self.services.store().conn().await?;
            let Some(state) = conn.subscriptions().extra_traffic(id).await? else {
                return Ok(false);
            };
            let Some(ends) = state.window_ends_at.filter(|at| *at <= now) else {
                return Ok(false);
            };
            let subscription = conn.subscriptions().get(id).await?;
            let user = match &subscription.remna {
                Some(link) if subscription.ended_at.is_none() => {
                    match remna.get_user(link.user_id).await {
                        Ok(user) => Some(user),
                        Err(err) if err.is_not_found() => None,
                        Err(err) => return Err(err.into()),
                    }
                }
                _ => None,
            };
            let Some(user) = user else {
                conn.subscriptions().clear_extra_traffic(id).await?;
                return Ok(true);
            };
            if window_mode(user.traffic_limit_strategy).is_none() {
                conn.subscriptions()
                    .set_extra_traffic(&ExtraTraffic {
                        window_started_at: None,
                        window_lifetime: None,
                        window_ends_at: None,
                        ..state
                    })
                    .await?;
                return Ok(true);
            }
            let start_lifetime = state.window_lifetime.unwrap_or_else(|| lifetime_at_reset(&user));
            let reset_seen = match (user.last_traffic_reset_at, state.window_started_at) {
                (Some(at), Some(start)) => at > start,
                (Some(_), None) => true,
                (None, _) => false,
            } || lifetime_at_reset(&user) > start_lifetime;
            if !reset_seen {
                if now < ends + Duration::hours(RESET_GRACE_HOURS) {
                    return Ok(false);
                }
                let next = self.window_for(&user, state.bytes, now);
                conn.subscriptions()
                    .set_extra_traffic(&ExtraTraffic {
                        window_ends_at: next.window_ends_at,
                        ..state
                    })
                    .await?;
                tracing::warn!(subscription = %id, "Remnawave не сбросила трафик в срок, сверка отложена");
                return Ok(false);
            }
            let limit = bytes_of(user.traffic_limit_bytes);
            let base = limit.saturating_sub(state.bytes);
            let spent = lifetime_at_reset(&user).saturating_sub(start_lifetime);
            let unused = unused_extra(limit, spent, state.bytes);
            let leftover = match subscription.tariff_id {
                Some(tariff_id) => match conn.tariffs().get(tariff_id).await {
                    Ok(tariff) => tariff.traffic_leftover(),
                    Err(err) if err.is_not_found() => TrafficLeftover::Burn,
                    Err(err) => return Err(err.into()),
                },
                None => TrafficLeftover::Burn,
            };
            drop(conn);
            let kept = if leftover.carries() { unused } else { 0 };
            let target = base.saturating_add(kept);
            let wake = user.status == UserStatus::Limited && traffic_left(target, &user);
            let after = if target == limit && !wake {
                user.clone()
            } else {
                let mut request = UpdateUserRequest::new(user.id);
                request.traffic_limit_bytes = Some(i64::try_from(target).unwrap_or(i64::MAX));
                if wake {
                    request.status = Some(UserStatus::Active);
                }
                match remna.update_user(&request).await {
                    Ok(updated) => updated,
                    Err(err) => {
                        confirm(&*remna, user.id, err, |u| bytes_of(u.traffic_limit_bytes) == target)
                            .await?
                    }
                }
            };
            let mut tx = self.services.store().begin().await?;
            let next = ExtraTraffic {
                subscription_id: id,
                ..self.window_for(&after, kept, now)
            };
            tx.subscriptions().set_extra_traffic(&next).await?;
            store_user(&mut tx, id, &after, None, now).await?;
            audit(
                &mut tx,
                &subscription,
                actions::EXTRA_TRAFFIC_SETTLED,
                json!({
                    "bought": state.bytes,
                    "spent_in_window": spent,
                    "kept": kept,
                    "burned": state.bytes.saturating_sub(kept),
                    "traffic_limit_before": limit,
                    "traffic_limit": bytes_of(after.traffic_limit_bytes),
                }),
                now,
            )
            .await?;
            tx.commit().await?;
            Ok(true)
        })
        .await
    }
}
