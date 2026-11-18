use super::{
    SubscriptionOps, confirm, disable_remna_user, drop_everywhere, enable_remna_user,
    with_subscription_lock,
};
use crate::error::{ServiceError, ServiceResult};
use crate::remna_sync::store_user;
use serde_json::json;
use sn_domain::ids::{ClientId, RemnaUserId, SubscriptionId};
use sn_modules::{Lifted, Restricted, TrafficReading, TrafficRestriction};
use sn_remnawave::{RemnaUser, UpdateUserRequest, UserStatus, UsersStreamQuery};
use sn_store::{ActorKind, AuditEntry, Db};
use std::collections::HashMap;
use uuid::Uuid;

const PAGE: u32 = 500;
const MAX_PAGES: usize = 20_000;

pub mod actions {
    pub const RESTRICTED: &str = "subscription.traffic_restricted";
    pub const LIFTED: &str = "subscription.traffic_lifted";
}

fn squads_of(user: &RemnaUser) -> Vec<Uuid> {
    user.active_internal_squads.iter().map(|s| s.uuid).collect()
}

impl SubscriptionOps<'_> {
    pub async fn traffic_readings(&self) -> ServiceResult<Vec<TrafficReading>> {
        let remna = self.services.remna()?;
        let links: HashMap<i64, i64> = {
            let mut conn = self.services.store().conn().await?;
            let rows: Vec<(i64, i64)> = sqlx::query_as(
                "SELECT remna_user_id, id FROM subscriptions \
                 WHERE ended_at IS NULL AND remna_user_id IS NOT NULL",
            )
            .fetch_all(conn.conn())
            .await?;
            rows.into_iter().collect()
        };
        let mut out = Vec::with_capacity(links.len());
        if links.is_empty() {
            return Ok(out);
        }
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let page = remna
                .stream_users(&UsersStreamQuery {
                    cursor: cursor.clone(),
                    size: Some(PAGE),
                    ..UsersStreamQuery::default()
                })
                .await?;
            let got = page.users.len();
            for user in &page.users {
                if let Some(id) = links.get(&user.id.0) {
                    out.push(TrafficReading {
                        subscription_id: SubscriptionId(*id),
                        lifetime_bytes: u64::try_from(
                            user.user_traffic.lifetime_used_traffic_bytes,
                        )
                        .unwrap_or(0),
                        status: user.status.into(),
                    });
                }
            }
            match page.next_cursor {
                Some(next) if page.has_more && got > 0 && Some(&next) != cursor.as_ref() => {
                    cursor = Some(next);
                }
                _ => break,
            }
        }
        Ok(out)
    }

    async fn traffic_target(
        &self,
        id: SubscriptionId,
    ) -> ServiceResult<Option<(ClientId, RemnaUserId, bool)>> {
        let mut conn = self.services.store().conn().await?;
        let subscription = conn.subscriptions().get(id).await?;
        let Some(link) = subscription.remna.as_ref() else {
            return Ok(None);
        };
        if subscription.ended_at.is_some() {
            return Ok(None);
        }
        let blocked = conn
            .clients()
            .get(subscription.client_id)
            .await?
            .is_blocked();
        Ok(Some((subscription.client_id, link.user_id, blocked)))
    }

    async fn traffic_audit(
        &self,
        id: SubscriptionId,
        user: &RemnaUser,
        action: &str,
        details: serde_json::Value,
    ) -> ServiceResult<()> {
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        store_user(&mut tx, id, user, None, now).await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::System,
                actor_id: None,
                action: action.to_string(),
                entity_kind: Some("subscription".into()),
                entity_id: Some(id.0),
                details,
                ip: None,
            })
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn restrict_traffic(
        &self,
        id: SubscriptionId,
        restriction: &TrafficRestriction,
    ) -> ServiceResult<Restricted> {
        let Some((_, user_id, blocked)) = self.traffic_target(id).await? else {
            return Ok(Restricted::Skipped("подписка закончилась".into()));
        };
        if blocked {
            return Ok(Restricted::Skipped("клиент заблокирован".into()));
        }
        let remna = self.services.remna()?;
        let result = with_subscription_lock(self.services, id, async {
            let current = remna.get_user(user_id).await?;
            if current.status != UserStatus::Active {
                return Ok::<_, ServiceError>(None);
            }
            match restriction {
                TrafficRestriction::Disable => {
                    let user = match disable_remna_user(&*remna, user_id).await {
                        Ok(user) => user,
                        Err(err) => {
                            confirm(&*remna, user_id, err, |u| u.status == UserStatus::Disabled)
                                .await?
                        }
                    };
                    Ok(Some((user, Vec::new())))
                }
                TrafficRestriction::Squad { squad, .. } => {
                    let previous = squads_of(&current);
                    if previous == [*squad] {
                        return Ok(Some((current, Vec::new())));
                    }
                    let mut request = UpdateUserRequest::new(user_id);
                    request.active_internal_squads = Some(vec![*squad]);
                    let user = match remna.update_user(&request).await {
                        Ok(user) => user,
                        Err(err) => {
                            let wanted = *squad;
                            confirm(&*remna, user_id, err, move |u| squads_of(u) == [wanted])
                                .await?
                        }
                    };
                    Ok(Some((user, previous)))
                }
            }
        })
        .await?;
        let Some((user, previous)) = result else {
            return Ok(Restricted::Skipped("подписка не активна".into()));
        };
        if let Err(err) = drop_everywhere(&*remna, vec![user_id]).await {
            tracing::warn!(subscription = %id, error = %err, "подключения ограниченной подписки не сброшены");
        }
        self.traffic_audit(
            id,
            &user,
            actions::RESTRICTED,
            json!({ "restriction": restriction, "previous_squads": previous }),
        )
        .await?;
        Ok(Restricted::Applied {
            previous_squads: previous,
        })
    }

    pub async fn lift_traffic(
        &self,
        id: SubscriptionId,
        restriction: &TrafficRestriction,
    ) -> ServiceResult<Lifted> {
        let Some((_, user_id, blocked)) = self.traffic_target(id).await? else {
            return Ok(Lifted::Ended);
        };
        if blocked && matches!(restriction, TrafficRestriction::Disable) {
            return Ok(Lifted::Blocked);
        }
        let remna = self.services.remna()?;
        let result = with_subscription_lock(self.services, id, async {
            let current = match remna.get_user(user_id).await {
                Ok(user) => user,
                Err(err) if err.is_not_found() => return Ok::<_, ServiceError>(Err(Lifted::Ended)),
                Err(err) => return Err(err.into()),
            };
            match restriction {
                TrafficRestriction::Disable => {
                    if current.status != UserStatus::Disabled {
                        return Ok(Err(Lifted::Changed(
                            "подписку уже включили другим действием".into(),
                        )));
                    }
                    let user = match enable_remna_user(&*remna, user_id).await {
                        Ok(user) => user,
                        Err(err) => {
                            confirm(&*remna, user_id, err, |u| u.status != UserStatus::Disabled)
                                .await?
                        }
                    };
                    Ok(Ok(user))
                }
                TrafficRestriction::Squad { squad, previous } => {
                    if squads_of(&current) != [*squad] {
                        return Ok(Err(Lifted::Changed(
                            "сквады подписки уже поменяли другим действием".into(),
                        )));
                    }
                    let mut request = UpdateUserRequest::new(user_id);
                    request.active_internal_squads = Some(previous.clone());
                    let wanted = previous.clone();
                    let user = match remna.update_user(&request).await {
                        Ok(user) => user,
                        Err(err) => {
                            confirm(&*remna, user_id, err, move |u| squads_of(u) == wanted).await?
                        }
                    };
                    Ok(Ok(user))
                }
            }
        })
        .await?;
        let user = match result {
            Ok(user) => user,
            Err(outcome) => return Ok(outcome),
        };
        self.traffic_audit(
            id,
            &user,
            actions::LIFTED,
            json!({ "restriction": restriction }),
        )
        .await?;
        Ok(Lifted::Restored)
    }

    pub async fn client_hwids(&self, client_id: ClientId) -> ServiceResult<Vec<String>> {
        let links: Vec<i64> = {
            let mut conn = self.services.store().conn().await?;
            sqlx::query_scalar(
                "SELECT remna_user_id FROM subscriptions WHERE client_id = $1 \
                 AND remna_user_id IS NOT NULL ORDER BY id DESC LIMIT 5",
            )
            .bind(client_id.0)
            .fetch_all(conn.conn())
            .await?
        };
        if links.is_empty() {
            return Ok(Vec::new());
        }
        let remna = self.services.remna()?;
        let mut out = Vec::new();
        for user_id in links {
            match remna.user_devices(RemnaUserId(user_id)).await {
                Ok(devices) => out.extend(devices.devices.into_iter().map(|d| d.hwid)),
                Err(err) if err.is_not_found() => {}
                Err(err) => return Err(err.into()),
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }
}
