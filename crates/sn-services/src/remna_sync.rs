use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::events::DomainEvent;
use sn_domain::ids::{RemnaUserId, SubscriptionId};
use sn_domain::jobs::JobPayload;
use sn_domain::subscription::{RemnaLink, RemnaMirror, Subscription, remna_description};
use sn_remnawave::webhook::{WebhookError, events, scopes, verify_and_parse};
use sn_remnawave::{RemnaError, RemnaUser};
use sn_store::{Db, NewRemnaEvent, RemnaEventRecord, Tx};
use std::collections::HashSet;

pub const RECONCILE_STALE_AFTER_MINUTES: i64 = 15;
pub const RECONCILE_BATCH: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemnaWebhookResult {
    Accepted { event_id: i64 },
    Duplicate,
    NotConfigured,
    BadSignature,
    Malformed { reason: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReconcileReport {
    pub checked: usize,
    pub updated: usize,
    pub missing: usize,
    pub failed: usize,
}

#[derive(Deserialize)]
struct UserEventData {
    user: RemnaUser,
}

#[derive(Deserialize)]
struct StoredEvent {
    data: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refreshed {
    Changed,
    Unchanged,
    Missing,
    NotLinked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Stored {
    pub applied: bool,
    pub changed: bool,
}

fn micros(at: Option<DateTime<Utc>>) -> Option<i64> {
    at.map(|at| at.timestamp_micros())
}

fn mirror_changed(old: &RemnaMirror, new: &RemnaMirror) -> bool {
    old.status != new.status
        || micros(old.expire_at) != micros(new.expire_at)
        || old.traffic_used_bytes != new.traffic_used_bytes
        || old.traffic_limit_bytes != new.traffic_limit_bytes
        || old.traffic_strategy != new.traffic_strategy
        || old.hwid_limit != new.hwid_limit
        || (new.devices_count.is_some() && old.devices_count != new.devices_count)
        || micros(old.online_at) != micros(new.online_at)
        || micros(old.first_connected_at) != micros(new.first_connected_at)
        || old.last_node != new.last_node
}

pub(crate) async fn store_user(
    tx: &mut Tx,
    subscription_id: SubscriptionId,
    user: &RemnaUser,
    devices: Option<u32>,
    now: DateTime<Utc>,
) -> ServiceResult<Stored> {
    let current = tx.subscriptions().lock(subscription_id).await?;
    let mirror = user.mirror(devices, now);
    let applied = tx
        .subscriptions()
        .apply_mirror(subscription_id, &mirror)
        .await?;
    if !applied {
        return Ok(Stored::default());
    }
    if let (Some(from), Some(to)) = (current.mirror.status, mirror.status)
        && from != to
    {
        tx.events()
            .push(&DomainEvent::SubscriptionStatusChanged {
                subscription_id,
                client_id: current.client_id,
                from: Some(from),
                to,
            })
            .await?;
    }
    if let Some(link) = &current.remna {
        let short_uuid = Some(user.short_uuid.clone()).filter(|s| !s.is_empty());
        let renamed = link.username != user.username;
        let reissued = short_uuid.is_some() && link.short_uuid != short_uuid;
        if renamed || reissued {
            tx.subscriptions()
                .link_remna(
                    subscription_id,
                    &RemnaLink {
                        user_id: user.id,
                        username: user.username.clone(),
                        short_uuid: short_uuid.or_else(|| link.short_uuid.clone()),
                    },
                )
                .await?;
        }
    }
    Ok(Stored {
        applied: true,
        changed: mirror_changed(&current.mirror, &mirror),
    })
}

fn remna_down(err: &ServiceError) -> bool {
    match err {
        ServiceError::RemnaNotConfigured(_) => true,
        ServiceError::Remna(err) => {
            err.outcome_unknown()
                || matches!(err, RemnaError::Unauthorized(_) | RemnaError::RateLimited)
        }
        _ => false,
    }
}

pub struct RemnaSync<'a> {
    services: &'a Services,
}

impl<'a> RemnaSync<'a> {
    pub fn new(services: &'a Services) -> Self {
        RemnaSync { services }
    }

    pub async fn accept_webhook(
        &self,
        signature: Option<&str>,
        body: &[u8],
    ) -> ServiceResult<RemnaWebhookResult> {
        let Some(secret) = self.services.remna_webhook_secret() else {
            return Ok(RemnaWebhookResult::NotConfigured);
        };
        let parsed = match verify_and_parse(&secret, body, signature) {
            Ok(parsed) => parsed,
            Err(WebhookError::BadSignature) => return Ok(RemnaWebhookResult::BadSignature),
            Err(err) => {
                return Ok(RemnaWebhookResult::Malformed {
                    reason: err.to_string(),
                });
            }
        };
        let subject = parsed.subject();
        let mut tx = self.services.store().begin().await?;
        let inserted = tx
            .remna_events()
            .insert(&NewRemnaEvent {
                body_sha256: parsed.digest.to_vec(),
                scope: parsed.scope.clone(),
                event: parsed.event.clone(),
                occurred_at: parsed.occurred_at,
                subject_kind: subject.as_ref().map(|(kind, _)| kind.to_string()),
                subject_id: subject.map(|(_, id)| id),
                payload: parsed.redacted_payload(),
            })
            .await?;
        let Some(event_id) = inserted else {
            return Ok(RemnaWebhookResult::Duplicate);
        };
        tx.jobs()
            .enqueue(&JobPayload::ProcessRemnaEvent { event_id }, None)
            .await?;
        tx.commit().await?;
        Ok(RemnaWebhookResult::Accepted { event_id })
    }

    pub async fn process_event(&self, event_id: i64) -> ServiceResult<()> {
        let record = {
            let mut conn = self.services.store().conn().await?;
            conn.remna_events().get(event_id).await?
        };
        if record.processed_at.is_some() {
            return Ok(());
        }
        match record.scope.as_str() {
            scopes::USER => self.user_event(&record).await?,
            scopes::USER_HWID_DEVICES => self.resync_subject(&record).await?,
            _ => {}
        }
        let mut conn = self.services.store().conn().await?;
        conn.remna_events().mark_processed(event_id).await?;
        Ok(())
    }

    async fn user_event(&self, record: &RemnaEventRecord) -> ServiceResult<()> {
        let event = record.event.as_str();
        let Some(user) = serde_json::from_value::<StoredEvent>(record.payload.clone())
            .ok()
            .and_then(|stored| serde_json::from_value::<UserEventData>(stored.data).ok())
            .map(|data| data.user)
        else {
            return self.resync_subject(record).await;
        };
        let mut tx = self.services.store().begin().await?;
        let subscription = match tx.subscriptions().by_remna_user(user.id).await? {
            Some(subscription) => Some(subscription),
            None if event == events::USER_DELETED => None,
            None => self.adopt(&mut tx, &user).await?,
        };
        let Some(subscription) = subscription else {
            tx.commit().await?;
            return Ok(());
        };
        let sync = JobPayload::SyncSubscription {
            subscription_id: subscription.id,
        };
        match event {
            events::USER_DELETED => {
                tx.subscriptions()
                    .mark_missing(subscription.id, true)
                    .await?;
            }
            events::USER_NOT_CONNECTED => {
                tx.jobs().enqueue(&sync, None).await?;
            }
            _ => {
                store_user(&mut tx, subscription.id, &user, None, self.services.now()).await?;
                let short_uuid_unknown = subscription
                    .remna
                    .as_ref()
                    .is_none_or(|link| link.short_uuid.is_none());
                if event == events::USER_REVOKED || short_uuid_unknown {
                    tx.jobs().enqueue(&sync, None).await?;
                }
            }
        }
        tx.commit().await?;
        Ok(())
    }

    async fn adopt(&self, tx: &mut Tx, user: &RemnaUser) -> ServiceResult<Option<Subscription>> {
        let Some(candidate) = tx.subscriptions().by_remna_username(&user.username).await? else {
            return Ok(None);
        };
        if candidate.remna.is_some() || candidate.ended_at.is_some() {
            return Ok(None);
        }
        let client = tx.clients().get(candidate.client_id).await?;
        if user.description.as_deref() != Some(remna_description(client.public_id).as_str()) {
            tracing::warn!(
                subscription = candidate.id.0,
                remna_user = user.id.0,
                "имя пользователя Remnawave совпало с зарезервированным, но описание чужое"
            );
            return Ok(None);
        }
        tx.subscriptions()
            .link_remna(
                candidate.id,
                &RemnaLink {
                    user_id: user.id,
                    username: user.username.clone(),
                    short_uuid: None,
                },
            )
            .await?;
        Ok(Some(tx.subscriptions().get(candidate.id).await?))
    }

    async fn resync_subject(&self, record: &RemnaEventRecord) -> ServiceResult<()> {
        let Some(user_id) = record
            .subject_id
            .as_deref()
            .filter(|_| record.subject_kind.as_deref() == Some("user"))
            .and_then(|id| id.parse::<i64>().ok())
            .map(RemnaUserId)
        else {
            return Ok(());
        };
        let mut tx = self.services.store().begin().await?;
        if let Some(subscription) = tx.subscriptions().by_remna_user(user_id).await? {
            tx.jobs()
                .enqueue(
                    &JobPayload::SyncSubscription {
                        subscription_id: subscription.id,
                    },
                    None,
                )
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn refresh_user(&self, user_id: RemnaUserId) -> ServiceResult<bool> {
        let remna = self.services.remna()?;
        let user = remna.get_user(user_id).await?;
        let mut tx = self.services.store().begin().await?;
        let Some(subscription) = tx.subscriptions().by_remna_user(user_id).await? else {
            tx.commit().await?;
            return Ok(false);
        };
        let stored = store_user(&mut tx, subscription.id, &user, None, self.services.now()).await?;
        tx.commit().await?;
        Ok(stored.applied)
    }

    pub async fn refresh_subscription(&self, id: SubscriptionId) -> ServiceResult<bool> {
        let subscription = {
            let mut conn = self.services.store().conn().await?;
            conn.subscriptions().get(id).await?
        };
        Ok(matches!(
            self.refresh(&subscription).await?,
            Refreshed::Changed | Refreshed::Unchanged
        ))
    }

    async fn refresh(&self, subscription: &Subscription) -> ServiceResult<Refreshed> {
        let Some(link) = &subscription.remna else {
            return Ok(Refreshed::NotLinked);
        };
        let remna = self.services.remna()?;
        let user = match remna.get_user(link.user_id).await {
            Ok(user) => user,
            Err(err) if err.is_not_found() => {
                let mut conn = self.services.store().conn().await?;
                conn.subscriptions()
                    .mark_missing(subscription.id, true)
                    .await?;
                return Ok(Refreshed::Missing);
            }
            Err(err) => return Err(err.into()),
        };
        let devices = remna.user_devices(link.user_id).await?;
        let count = u32::try_from(devices.total.max(0)).unwrap_or(u32::MAX);
        let mut tx = self.services.store().begin().await?;
        tx.subscriptions()
            .mark_missing(subscription.id, false)
            .await?;
        let stored = store_user(
            &mut tx,
            subscription.id,
            &user,
            Some(count),
            self.services.now(),
        )
        .await?;
        tx.commit().await?;
        Ok(if stored.changed {
            Refreshed::Changed
        } else {
            Refreshed::Unchanged
        })
    }

    pub async fn reconcile(&self, limit: usize) -> ServiceResult<ReconcileReport> {
        let cutoff = self.services.now() - Duration::minutes(RECONCILE_STALE_AFTER_MINUTES);
        let mut report = ReconcileReport::default();
        let mut seen: HashSet<SubscriptionId> = HashSet::new();
        while report.checked < limit {
            let wanted = (limit - report.checked).min(RECONCILE_BATCH);
            let batch = {
                let mut conn = self.services.store().conn().await?;
                conn.subscriptions()
                    .stale_mirrors(cutoff, i64::try_from(wanted).unwrap_or(i64::MAX))
                    .await?
            };
            let fresh: Vec<Subscription> = batch
                .into_iter()
                .filter(|subscription| seen.insert(subscription.id))
                .collect();
            if fresh.is_empty() {
                break;
            }
            for subscription in fresh {
                report.checked += 1;
                match self.refresh(&subscription).await {
                    Ok(Refreshed::Changed) => report.updated += 1,
                    Ok(Refreshed::Missing) => report.missing += 1,
                    Ok(Refreshed::Unchanged | Refreshed::NotLinked) => {}
                    Err(err) => {
                        report.failed += 1;
                        tracing::warn!(
                            subscription = subscription.id.0,
                            error = %err,
                            "сверка подписки с Remnawave не удалась"
                        );
                        if remna_down(&err) {
                            return Ok(report);
                        }
                    }
                }
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscriptions::tests::{Env, env};
    use sn_domain::subscription::AutoRenewState;
    use sn_domain::subscription::{SubscriptionKind, SubscriptionStatus, remna_username};
    use sn_remnawave::fake::{Fault, sample_device, sample_user, user_webhook, webhook_body};
    use sn_remnawave::webhook::sign;
    use sn_remnawave::{CreateUserRequest, RemnawaveApi, UpdateUserRequest, UserStatus};
    use sn_store::NewSubscription;
    use std::time::Duration as StdDuration;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    async fn deliver(env: &Env, body: &[u8]) -> i64 {
        env.services.set_remna_webhook_secret(Some(SECRET.into()));
        let signature = sign(SECRET, body);
        let RemnaWebhookResult::Accepted { event_id } = env
            .services
            .remna_sync()
            .accept_webhook(Some(&signature), body)
            .await
            .unwrap()
        else {
            panic!("событие не принято");
        };
        env.services
            .remna_sync()
            .process_event(event_id)
            .await
            .unwrap();
        event_id
    }

    async fn sync_jobs(env: &Env) -> Vec<SubscriptionId> {
        let mut conn = env.db.store.conn().await.unwrap();
        conn.jobs()
            .claim(50, StdDuration::from_secs(30))
            .await
            .unwrap()
            .into_iter()
            .filter_map(|job| match job.typed().ok()? {
                JobPayload::SyncSubscription { subscription_id } => Some(subscription_id),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn обновление_подписки_берёт_пользователя_и_устройства() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let subscription = env.linked(client.id, sample_user(1, "sn_refresh")).await;
        env.fake
            .add_device(sample_device(RemnaUserId(1), "hw-1", Utc::now()));
        env.fake
            .add_device(sample_device(RemnaUserId(1), "hw-2", Utc::now()));
        let mut patch = UpdateUserRequest::new(RemnaUserId(1));
        patch.expire_at = Some(Utc::now() + Duration::days(90));
        let changed = env.fake.update_user(&patch).await.unwrap();
        let sync = env.services.remna_sync();
        assert!(sync.refresh_subscription(subscription.id).await.unwrap());
        let stored = env.subscription(subscription.id).await;
        assert_eq!(stored.mirror.devices_count, Some(2));
        assert_eq!(
            stored.mirror.expire_at.map(|t| t.timestamp_micros()),
            Some(changed.expire_at.timestamp_micros())
        );
        env.fake.delete_user(RemnaUserId(1)).await.unwrap();
        assert!(!sync.refresh_subscription(subscription.id).await.unwrap());
        env.fake.fail_next(Fault::Unavailable);
        assert!(sync.refresh_subscription(subscription.id).await.is_err());
        env.finish().await;
    }

    #[tokio::test]
    async fn сверка_проходит_устаревшие_зеркала_и_пишет_смену_статуса() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let same = env.linked(client.id, sample_user(1, "sn_same")).await;
        let limited = env.linked(client.id, sample_user(2, "sn_limited")).await;
        let gone = env.linked(client.id, sample_user(3, "sn_gone")).await;
        let mut user = env.fake.user(RemnaUserId(2)).unwrap();
        user.status = UserStatus::Limited;
        user.updated_at = Utc::now();
        env.fake.insert_user(user);
        env.fake.delete_user(RemnaUserId(3)).await.unwrap();
        let sync = env.services.remna_sync();
        let report = sync.reconcile(10).await.unwrap();
        assert_eq!(report.checked, 3);
        assert_eq!(report.missing, 1);
        assert_eq!(report.failed, 0);
        assert!(report.updated >= 1);
        assert_eq!(
            env.subscription(limited.id).await.mirror.status,
            Some(SubscriptionStatus::Limited)
        );
        assert_eq!(
            env.status_events(client.id).await,
            vec![(
                Some(SubscriptionStatus::Active),
                SubscriptionStatus::Limited
            )]
        );
        assert!(env.subscription(same.id).await.mirror.synced_at.is_some());
        assert!(env.subscription(gone.id).await.remna.is_some());
        let again = sync.reconcile(10).await.unwrap();
        assert_eq!(again.checked, 0);
        env.finish().await;
    }

    #[tokio::test]
    async fn сверка_останавливается_когда_панель_недоступна() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        for id in 1..=3 {
            env.linked(client.id, sample_user(id, &format!("sn_down_{id}")))
                .await;
        }
        env.fake.fail_next(Fault::Unavailable);
        let report = env.services.remna_sync().reconcile(10).await.unwrap();
        assert_eq!(report.checked, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(env.fake.calls().len(), 1);
        let limited = env.services.remna_sync().reconcile(2).await.unwrap();
        assert_eq!(limited.checked, 2);
        assert_eq!(limited.failed, 0);
        env.finish().await;
    }

    #[tokio::test]
    async fn событие_пользователя_меняет_зеркало_а_ноды_и_оплаты_нет() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let subscription = env.linked(client.id, sample_user(1, "sn_event")).await;
        let mut user = env.fake.user(RemnaUserId(1)).unwrap();
        user.expire_at = Utc::now() + Duration::days(200);
        user.status = UserStatus::Disabled;
        user.updated_at = Utc::now();
        deliver(
            &env,
            &user_webhook(events::USER_DISABLED, &user, Utc::now()),
        )
        .await;
        let stored = env.subscription(subscription.id).await;
        assert_eq!(stored.mirror.status, Some(SubscriptionStatus::Disabled));
        assert_eq!(
            stored.mirror.expire_at.map(|t| t.timestamp_micros()),
            Some(user.expire_at.timestamp_micros())
        );
        assert_eq!(
            env.status_events(client.id).await,
            vec![(
                Some(SubscriptionStatus::Active),
                SubscriptionStatus::Disabled
            )]
        );

        let before = env.subscription(subscription.id).await;
        let node = webhook_body(
            scopes::NODE,
            events::NODE_CONNECTION_LOST,
            Utc::now(),
            serde_json::json!({"uuid": "00000000-0000-4000-8000-000000000002", "name": "NL-2",
                               "address": "198.51.100.22", "isConnected": false, "isDisabled": false}),
        );
        let crm = webhook_body(
            scopes::CRM,
            events::CRM_PAYMENT_IN_7_DAYS,
            Utc::now(),
            serde_json::json!({"providerName": "Hetzner", "nodeName": "FI-1",
                               "nextBillingAt": "2026-11-18T00:00:00.000Z", "loginUrl": "https://hetzner.example"}),
        );
        for body in [node, crm] {
            let id = deliver(&env, &body).await;
            let mut conn = env.db.store.conn().await.unwrap();
            assert!(
                conn.remna_events()
                    .get(id)
                    .await
                    .unwrap()
                    .processed_at
                    .is_some()
            );
        }
        assert_eq!(env.subscription(subscription.id).await, before);

        let mut old = user.clone();
        old.status = UserStatus::Active;
        old.updated_at = user.updated_at - Duration::minutes(5);
        deliver(&env, &user_webhook(events::USER_ENABLED, &old, Utc::now())).await;
        assert_eq!(
            env.subscription(subscription.id).await.mirror.status,
            Some(SubscriptionStatus::Disabled)
        );
        let mut conn = env.db.store.conn().await.unwrap();
        let feed = conn.remna_events().feed(None, 10).await.unwrap();
        assert!(
            !feed
                .iter()
                .any(|e| e.payload.to_string().contains("fake-trojan-password"))
        );
        assert!(
            !feed
                .iter()
                .any(|e| e.payload.to_string().contains("sub.example.com"))
        );
        drop(conn);
        env.finish().await;
    }

    #[tokio::test]
    async fn событие_устройства_ставит_сверку_и_не_затирает_ноду() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let node = uuid::Uuid::from_u128(77);
        let mut user = sample_user(1, "sn_hwid");
        user.user_traffic.last_connected_node_uuid = Some(node);
        let subscription = env.linked(client.id, user.clone()).await;
        assert_eq!(subscription.mirror.last_node, Some(node));
        let mut partial = user.clone();
        partial.user_traffic.last_connected_node_uuid = None;
        partial.updated_at = Utc::now() + Duration::minutes(1);
        let device = sample_device(RemnaUserId(1), "hw-9", Utc::now());
        let body = webhook_body(
            scopes::USER_HWID_DEVICES,
            events::HWID_ADDED,
            Utc::now(),
            serde_json::json!({"user": partial, "hwidUserDevice": device}),
        );
        deliver(&env, &body).await;
        assert_eq!(
            env.subscription(subscription.id).await.mirror.last_node,
            Some(node)
        );
        assert_eq!(sync_jobs(&env).await, vec![subscription.id]);

        deliver(
            &env,
            &user_webhook(events::USER_NOT_CONNECTED, &partial, Utc::now()),
        )
        .await;
        assert_eq!(
            env.subscription(subscription.id).await.mirror.last_node,
            Some(node)
        );
        assert_eq!(sync_jobs(&env).await, vec![subscription.id]);
        env.finish().await;
    }

    #[tokio::test]
    async fn неразобранное_событие_пользователя_ставит_сверку() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let subscription = env.linked(client.id, sample_user(1, "sn_drift")).await;
        let body = webhook_body(
            scopes::USER,
            events::USER_MODIFIED,
            Utc::now(),
            serde_json::json!({"id": 1, "status": "SOMETHING_NEW"}),
        );
        deliver(&env, &body).await;
        assert_eq!(sync_jobs(&env).await, vec![subscription.id]);
        assert_eq!(
            env.subscription(subscription.id).await.mirror.status,
            subscription.mirror.status
        );
        env.finish().await;
    }

    #[tokio::test]
    async fn удаление_пользователя_в_панели_помечает_подписку() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let subscription = env.linked(client.id, sample_user(1, "sn_deleted")).await;
        let user = env.fake.user(RemnaUserId(1)).unwrap();
        deliver(&env, &user_webhook(events::USER_DELETED, &user, Utc::now())).await;
        let stored = env.subscription(subscription.id).await;
        assert_eq!(stored.remna.map(|l| l.user_id), Some(RemnaUserId(1)));
        assert_eq!(stored.mirror.status, subscription.mirror.status);
        assert_eq!(stored.mirror.expire_at, subscription.mirror.expire_at);
        assert!(stored.mirror.synced_at > subscription.mirror.synced_at);
        let report = env.services.remna_sync().reconcile(10).await.unwrap();
        assert_eq!(report.checked, 0);
        assert!(env.fake.calls().iter().all(|c| c != "get_user"));
        env.finish().await;
    }

    #[tokio::test]
    async fn пользователь_создан_раньше_ответа_на_post_и_связывается_по_имени() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let subscription = {
            let mut tx = env.db.store.begin().await.unwrap();
            let subscription = tx
                .subscriptions()
                .create(&NewSubscription {
                    client_id: client.id,
                    tariff_id: None,
                    kind: SubscriptionKind::Paid,
                    extra_devices: 0,
                    autorenew: AutoRenewState::default(),
                })
                .await
                .unwrap();
            tx.subscriptions()
                .reserve_username(subscription.id, &remna_username(subscription.public_id))
                .await
                .unwrap();
            tx.commit().await.unwrap();
            subscription
        };
        env.services.set_remna_webhook_secret(Some(SECRET.into()));
        let hook_services = env.services.clone();
        env.fake.before_create_response(move |user| {
            let services = hook_services.clone();
            Box::pin(async move {
                let body = user_webhook(events::USER_CREATED, &user, Utc::now());
                let signature = sign(SECRET, &body);
                if let Ok(RemnaWebhookResult::Accepted { event_id }) = services
                    .remna_sync()
                    .accept_webhook(Some(&signature), &body)
                    .await
                {
                    let processed = services.remna_sync().process_event(event_id).await;
                    assert!(processed.is_ok());
                }
            })
        });
        let request = CreateUserRequest {
            username: remna_username(subscription.public_id),
            status: None,
            expire_at: Utc::now() + Duration::days(30),
            traffic_limit_bytes: 0,
            traffic_limit_strategy: sn_domain::traffic::TrafficStrategy::NoReset,
            description: Some(remna_description(client.public_id)),
            tag: None,
            telegram_id: None,
            email: None,
            hwid_device_limit: Some(3),
            active_internal_squads: Vec::new(),
            external_squad_uuid: None,
        };
        let created = env.fake.create_user(&request).await.unwrap();
        let stored = env.subscription(subscription.id).await;
        let link = stored.remna.clone().unwrap();
        assert_eq!(link.user_id, created.id);
        assert_eq!(link.short_uuid, None);
        assert_eq!(stored.mirror.status, Some(SubscriptionStatus::Active));
        assert_eq!(sync_jobs(&env).await, vec![subscription.id]);
        assert!(
            env.services
                .remna_sync()
                .refresh_subscription(subscription.id)
                .await
                .unwrap()
        );
        assert_eq!(
            env.subscription(subscription.id)
                .await
                .remna
                .unwrap()
                .short_uuid,
            Some(created.short_uuid.clone())
        );

        let stranger = {
            let mut tx = env.db.store.begin().await.unwrap();
            let other = tx
                .subscriptions()
                .create(&NewSubscription {
                    client_id: client.id,
                    tariff_id: None,
                    kind: SubscriptionKind::Paid,
                    extra_devices: 0,
                    autorenew: AutoRenewState::default(),
                })
                .await
                .unwrap();
            tx.subscriptions()
                .reserve_username(other.id, &remna_username(other.public_id))
                .await
                .unwrap();
            tx.commit().await.unwrap();
            other
        };
        let mut foreign = request.clone();
        foreign.username = remna_username(stranger.public_id);
        foreign.description = Some("чужой".into());
        env.fake.create_user(&foreign).await.unwrap();
        assert!(env.subscription(stranger.id).await.remna.is_none());
        env.finish().await;
    }

    #[tokio::test]
    async fn обновление_по_пользователю_пишет_событие_статуса() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let subscription = env.linked(client.id, sample_user(1, "sn_user")).await;
        env.fake.disable_user(RemnaUserId(1)).await.unwrap();
        assert!(
            env.services
                .remna_sync()
                .refresh_user(RemnaUserId(1))
                .await
                .unwrap()
        );
        assert_eq!(
            env.subscription(subscription.id).await.mirror.status,
            Some(SubscriptionStatus::Disabled)
        );
        assert_eq!(
            env.status_events(client.id).await,
            vec![(
                Some(SubscriptionStatus::Active),
                SubscriptionStatus::Disabled
            )]
        );
        assert!(
            !env.services
                .remna_sync()
                .refresh_user(RemnaUserId(99))
                .await
                .is_ok_and(|found| found)
        );
        env.finish().await;
    }
}
