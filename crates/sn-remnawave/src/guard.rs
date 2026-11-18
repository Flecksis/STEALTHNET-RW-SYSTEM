use crate::api::RemnawaveApi;
use crate::error::{RemnaError, RemnaResult};
use crate::models::*;
use async_trait::async_trait;
use sn_domain::ids::RemnaUserId;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const BREAKER_OPEN: &str = "предохранитель";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakerConfig {
    pub failure_threshold: u32,
    pub open_for: Duration,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        BreakerConfig {
            failure_threshold: 3,
            open_for: Duration::from_secs(30),
        }
    }
}

pub trait MonotonicClock: Send + Sync {
    fn now(&self) -> Instant;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemMonotonic;

impl MonotonicClock for SystemMonotonic {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone, Copy)]
enum Phase {
    Closed { failures: u32 },
    Open { until: Instant },
    Probing { since: Instant },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Success,
    Failure,
    Neutral,
}

fn verdict<T>(result: &RemnaResult<T>) -> Verdict {
    match result {
        Ok(_) => Verdict::Success,
        Err(
            RemnaError::Server { .. }
            | RemnaError::Unauthorized(_)
            | RemnaError::RateLimited
            | RemnaError::Timeout
            | RemnaError::Unavailable(_)
            | RemnaError::Dropped
            | RemnaError::Redirected(_),
        ) => Verdict::Failure,
        Err(RemnaError::NotFound { .. } | RemnaError::BadRequest { .. }) => Verdict::Success,
        Err(RemnaError::Decode(_) | RemnaError::NotConfigured(_) | RemnaError::Unsupported(_)) => {
            Verdict::Neutral
        }
    }
}

fn open_error() -> RemnaError {
    RemnaError::Unavailable(BREAKER_OPEN.to_string())
}

pub struct GuardedRemnawave<T> {
    inner: T,
    config: BreakerConfig,
    clock: Arc<dyn MonotonicClock>,
    phase: Mutex<Phase>,
}

impl<T: RemnawaveApi> GuardedRemnawave<T> {
    pub fn new(inner: T) -> Self {
        Self::with_config(inner, BreakerConfig::default(), Arc::new(SystemMonotonic))
    }

    pub fn with_config(inner: T, config: BreakerConfig, clock: Arc<dyn MonotonicClock>) -> Self {
        GuardedRemnawave {
            inner,
            config,
            clock,
            phase: Mutex::new(Phase::Closed { failures: 0 }),
        }
    }

    pub fn inner(&self) -> &T {
        &self.inner
    }

    pub fn state(&self) -> BreakerState {
        let now = self.clock.now();
        match *self.phase() {
            Phase::Closed { .. } => BreakerState::Closed,
            Phase::Open { until } if now < until => BreakerState::Open,
            Phase::Open { .. } | Phase::Probing { .. } => BreakerState::HalfOpen,
        }
    }

    fn phase(&self) -> MutexGuard<'_, Phase> {
        match self.phase.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn admit(&self) -> RemnaResult<()> {
        let now = self.clock.now();
        let mut phase = self.phase();
        match *phase {
            Phase::Closed { .. } => Ok(()),
            Phase::Open { until } if now < until => Err(open_error()),
            Phase::Probing { since } if now.duration_since(since) < self.config.open_for => {
                Err(open_error())
            }
            Phase::Open { .. } | Phase::Probing { .. } => {
                *phase = Phase::Probing { since: now };
                Ok(())
            }
        }
    }

    fn settle(&self, verdict: Verdict) {
        let now = self.clock.now();
        let mut phase = self.phase();
        *phase = match (verdict, *phase) {
            (Verdict::Success, _) => Phase::Closed { failures: 0 },
            (Verdict::Neutral, Phase::Probing { .. }) => Phase::Open { until: now },
            (Verdict::Neutral, current) => current,
            (Verdict::Failure, Phase::Closed { failures }) => {
                let failures = failures.saturating_add(1);
                if failures >= self.config.failure_threshold {
                    Phase::Open {
                        until: now + self.config.open_for,
                    }
                } else {
                    Phase::Closed { failures }
                }
            }
            (Verdict::Failure, Phase::Probing { .. }) => Phase::Open {
                until: now + self.config.open_for,
            },
            (Verdict::Failure, current @ Phase::Open { .. }) => current,
        };
    }

    async fn write<R, F, Fut>(&self, call: F) -> RemnaResult<R>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = RemnaResult<R>>,
    {
        self.admit()?;
        let result = call().await;
        self.settle(verdict(&result));
        result
    }

    async fn read<R, F, Fut>(&self, call: F) -> RemnaResult<R>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = RemnaResult<R>>,
    {
        self.admit()?;
        let mut result = call().await;
        if matches!(
            result,
            Err(RemnaError::Unavailable(_) | RemnaError::Dropped)
        ) {
            result = call().await;
        }
        self.settle(verdict(&result));
        result
    }
}

#[async_trait]
impl<T: RemnawaveApi> RemnawaveApi for GuardedRemnawave<T> {
    async fn metadata(&self) -> RemnaResult<SystemMetadata> {
        self.read(|| self.inner.metadata()).await
    }

    async fn configuration(&self) -> RemnaResult<SystemConfiguration> {
        self.read(|| self.inner.configuration()).await
    }

    async fn auth_status(&self) -> RemnaResult<()> {
        self.read(|| self.inner.auth_status()).await
    }

    async fn stats(&self) -> RemnaResult<SystemStats> {
        self.read(|| self.inner.stats()).await
    }

    async fn get_user(&self, id: RemnaUserId) -> RemnaResult<RemnaUser> {
        self.read(|| self.inner.get_user(id)).await
    }

    async fn get_user_by_username(&self, username: &str) -> RemnaResult<RemnaUser> {
        self.read(|| self.inner.get_user_by_username(username))
            .await
    }

    async fn get_user_by_short_uuid(&self, short_uuid: &str) -> RemnaResult<RemnaUser> {
        self.read(|| self.inner.get_user_by_short_uuid(short_uuid))
            .await
    }

    async fn stream_users(&self, query: &UsersStreamQuery) -> RemnaResult<UsersPage> {
        self.read(|| self.inner.stream_users(query)).await
    }

    async fn create_user(&self, request: &CreateUserRequest) -> RemnaResult<RemnaUser> {
        self.write(|| self.inner.create_user(request)).await
    }

    async fn update_user(&self, request: &UpdateUserRequest) -> RemnaResult<RemnaUser> {
        self.write(|| self.inner.update_user(request)).await
    }

    async fn delete_user(&self, id: RemnaUserId) -> RemnaResult<()> {
        self.write(|| self.inner.delete_user(id)).await
    }

    async fn extend_user(&self, id: RemnaUserId, days: u32) -> RemnaResult<RemnaUser> {
        self.write(|| self.inner.extend_user(id, days)).await
    }

    async fn reset_user_traffic(&self, id: RemnaUserId) -> RemnaResult<RemnaUser> {
        self.write(|| self.inner.reset_user_traffic(id)).await
    }

    async fn disable_user(&self, id: RemnaUserId) -> RemnaResult<RemnaUser> {
        self.write(|| self.inner.disable_user(id)).await
    }

    async fn enable_user(&self, id: RemnaUserId) -> RemnaResult<RemnaUser> {
        self.write(|| self.inner.enable_user(id)).await
    }

    async fn revoke_user(
        &self,
        id: RemnaUserId,
        request: &RevokeRequest,
    ) -> RemnaResult<RemnaUser> {
        self.write(|| self.inner.revoke_user(id, request)).await
    }

    async fn user_accessible_nodes(&self, id: RemnaUserId) -> RemnaResult<UserAccessibleNodes> {
        self.read(|| self.inner.user_accessible_nodes(id)).await
    }

    async fn user_subscription_requests(
        &self,
        id: RemnaUserId,
    ) -> RemnaResult<SubscriptionRequests> {
        self.read(|| self.inner.user_subscription_requests(id))
            .await
    }

    async fn user_usage(&self, id: RemnaUserId, range: DateRange) -> RemnaResult<UserUsage> {
        self.read(|| self.inner.user_usage(id, range)).await
    }

    async fn user_devices(&self, id: RemnaUserId) -> RemnaResult<HwidDevices> {
        self.read(|| self.inner.user_devices(id)).await
    }

    async fn delete_user_device(&self, id: RemnaUserId, hwid: &str) -> RemnaResult<HwidDevices> {
        self.write(|| self.inner.delete_user_device(id, hwid)).await
    }

    async fn delete_all_user_devices(&self, id: RemnaUserId) -> RemnaResult<HwidDevices> {
        self.write(|| self.inner.delete_all_user_devices(id)).await
    }

    async fn drop_connections(&self, request: &DropConnectionsRequest) -> RemnaResult<()> {
        self.write(|| self.inner.drop_connections(request)).await
    }

    async fn request_user_connections(&self, id: RemnaUserId) -> RemnaResult<ConnectionsJob> {
        self.write(|| self.inner.request_user_connections(id)).await
    }

    async fn user_connections(&self, job_id: &str) -> RemnaResult<UserConnectionsStatus> {
        self.read(|| self.inner.user_connections(job_id)).await
    }

    async fn bulk_update_users(&self, request: &BulkUpdateRequest) -> RemnaResult<()> {
        self.write(|| self.inner.bulk_update_users(request)).await
    }

    async fn bulk_update_squads(&self, request: &BulkSquadsRequest) -> RemnaResult<()> {
        self.write(|| self.inner.bulk_update_squads(request)).await
    }

    async fn bulk_extend_users(&self, request: &BulkExtendRequest) -> RemnaResult<()> {
        self.write(|| self.inner.bulk_extend_users(request)).await
    }

    async fn bulk_reset_traffic(&self, request: &BulkUsersRequest) -> RemnaResult<()> {
        self.write(|| self.inner.bulk_reset_traffic(request)).await
    }

    async fn nodes(&self) -> RemnaResult<Vec<Node>> {
        self.read(|| self.inner.nodes()).await
    }

    async fn restart_node(&self, uuid: Uuid) -> RemnaResult<()> {
        self.write(|| self.inner.restart_node(uuid)).await
    }

    async fn enable_node(&self, uuid: Uuid) -> RemnaResult<Node> {
        self.write(|| self.inner.enable_node(uuid)).await
    }

    async fn disable_node(&self, uuid: Uuid) -> RemnaResult<Node> {
        self.write(|| self.inner.disable_node(uuid)).await
    }

    async fn internal_squads(&self) -> RemnaResult<InternalSquads> {
        self.read(|| self.inner.internal_squads()).await
    }

    async fn squad_accessible_nodes(&self, uuid: Uuid) -> RemnaResult<SquadAccessibleNodes> {
        self.read(|| self.inner.squad_accessible_nodes(uuid)).await
    }

    async fn subscription_settings(&self) -> RemnaResult<SubscriptionSettings> {
        self.read(|| self.inner.subscription_settings()).await
    }

    async fn update_subscription_settings(
        &self,
        request: &UpdateSubscriptionSettingsRequest,
    ) -> RemnaResult<SubscriptionSettings> {
        self.write(|| self.inner.update_subscription_settings(request))
            .await
    }

    async fn infra_billing_nodes(&self) -> RemnaResult<InfraBillingNodes> {
        self.read(|| self.inner.infra_billing_nodes()).await
    }

    async fn node(&self, uuid: Uuid) -> RemnaResult<NodeDetails> {
        self.read(|| self.inner.node(uuid)).await
    }

    async fn nodes_detailed(&self) -> RemnaResult<Vec<NodeDetails>> {
        self.read(|| self.inner.nodes_detailed()).await
    }

    async fn create_node(&self, request: &CreateNodeRequest) -> RemnaResult<NodeDetails> {
        self.write(|| self.inner.create_node(request)).await
    }

    async fn update_node(&self, request: &UpdateNodeRequest) -> RemnaResult<NodeDetails> {
        self.write(|| self.inner.update_node(request)).await
    }

    async fn delete_node(&self, uuid: Uuid) -> RemnaResult<()> {
        self.write(|| self.inner.delete_node(uuid)).await
    }

    async fn reset_node_traffic(&self, uuid: Uuid) -> RemnaResult<()> {
        self.write(|| self.inner.reset_node_traffic(uuid)).await
    }

    async fn restart_all_nodes(&self, force: bool) -> RemnaResult<()> {
        self.write(|| self.inner.restart_all_nodes(force)).await
    }

    async fn keygen(&self) -> RemnaResult<NodeKeygen> {
        self.read(|| self.inner.keygen()).await
    }

    async fn config_profiles(&self) -> RemnaResult<ConfigProfiles> {
        self.read(|| self.inner.config_profiles()).await
    }

    async fn hosts(&self) -> RemnaResult<Vec<Host>> {
        self.read(|| self.inner.hosts()).await
    }

    async fn update_host(&self, request: &UpdateHostRequest) -> RemnaResult<Host> {
        self.write(|| self.inner.update_host(request)).await
    }

    async fn create_host(&self, request: &CreateHostRequest) -> RemnaResult<Host> {
        self.write(|| self.inner.create_host(request)).await
    }

    async fn delete_host(&self, uuid: Uuid) -> RemnaResult<()> {
        self.write(|| self.inner.delete_host(uuid)).await
    }

    async fn reorder_hosts(&self, positions: &[HostPosition]) -> RemnaResult<()> {
        self.write(|| self.inner.reorder_hosts(positions)).await
    }

    async fn enable_hosts(&self, uuids: &[Uuid]) -> RemnaResult<()> {
        self.write(|| self.inner.enable_hosts(uuids)).await
    }

    async fn disable_hosts(&self, uuids: &[Uuid]) -> RemnaResult<()> {
        self.write(|| self.inner.disable_hosts(uuids)).await
    }

    async fn create_internal_squad(
        &self,
        request: &CreateInternalSquadRequest,
    ) -> RemnaResult<InternalSquad> {
        self.write(|| self.inner.create_internal_squad(request))
            .await
    }

    async fn update_internal_squad(
        &self,
        request: &UpdateInternalSquadRequest,
    ) -> RemnaResult<InternalSquad> {
        self.write(|| self.inner.update_internal_squad(request))
            .await
    }

    async fn delete_internal_squad(&self, uuid: Uuid) -> RemnaResult<()> {
        self.write(|| self.inner.delete_internal_squad(uuid)).await
    }

    async fn internal_squad_usage(
        &self,
        uuid: Uuid,
        query: &SquadUsageQuery,
    ) -> RemnaResult<SquadUsage> {
        self.read(|| self.inner.internal_squad_usage(uuid, query))
            .await
    }

    async fn external_squads(&self) -> RemnaResult<ExternalSquads> {
        self.read(|| self.inner.external_squads()).await
    }

    async fn infra_providers(&self) -> RemnaResult<InfraProviders> {
        self.read(|| self.inner.infra_providers()).await
    }

    async fn infra_provider(&self, uuid: Uuid) -> RemnaResult<InfraProvider> {
        self.read(|| self.inner.infra_provider(uuid)).await
    }

    async fn create_infra_provider(
        &self,
        request: &CreateInfraProviderRequest,
    ) -> RemnaResult<InfraProvider> {
        self.write(|| self.inner.create_infra_provider(request))
            .await
    }

    async fn update_infra_provider(
        &self,
        request: &UpdateInfraProviderRequest,
    ) -> RemnaResult<InfraProvider> {
        self.write(|| self.inner.update_infra_provider(request))
            .await
    }

    async fn delete_infra_provider(&self, uuid: Uuid) -> RemnaResult<()> {
        self.write(|| self.inner.delete_infra_provider(uuid)).await
    }

    async fn infra_billing_history(
        &self,
        query: &BillingHistoryQuery,
    ) -> RemnaResult<InfraBillingHistory> {
        self.read(|| self.inner.infra_billing_history(query)).await
    }

    async fn create_infra_billing_record(
        &self,
        request: &CreateBillingRecordRequest,
    ) -> RemnaResult<InfraBillingHistory> {
        self.write(|| self.inner.create_infra_billing_record(request))
            .await
    }

    async fn create_infra_billing_node(
        &self,
        request: &CreateBillingNodeRequest,
    ) -> RemnaResult<InfraBillingNodes> {
        self.write(|| self.inner.create_infra_billing_node(request))
            .await
    }

    async fn update_infra_billing_nodes(
        &self,
        request: &UpdateBillingNodesRequest,
    ) -> RemnaResult<InfraBillingNodes> {
        self.write(|| self.inner.update_infra_billing_nodes(request))
            .await
    }

    async fn node_metadata(&self, uuid: Uuid) -> RemnaResult<NodeMetadata> {
        self.read(|| self.inner.node_metadata(uuid)).await
    }

    async fn set_node_metadata(
        &self,
        uuid: Uuid,
        metadata: &NodeMetadata,
    ) -> RemnaResult<NodeMetadata> {
        self.write(|| self.inner.set_node_metadata(uuid, metadata))
            .await
    }

    async fn subscription_page_configs(&self) -> RemnaResult<SubpageConfigs> {
        self.read(|| self.inner.subscription_page_configs()).await
    }

    async fn subscription_page_config(&self, uuid: Uuid) -> RemnaResult<SubpageConfig> {
        self.read(|| self.inner.subscription_page_config(uuid))
            .await
    }

    async fn subpage_access(&self, short_uuid: &str) -> RemnaResult<SubpageAccess> {
        self.read(|| self.inner.subpage_access(short_uuid)).await
    }

    async fn request_node_connections(&self, uuid: Uuid) -> RemnaResult<ConnectionsJob> {
        self.write(|| self.inner.request_node_connections(uuid))
            .await
    }

    async fn node_connections(&self, job_id: &str) -> RemnaResult<NodeConnectionsStatus> {
        self.read(|| self.inner.node_connections(job_id)).await
    }

    async fn nodes_usage(&self, query: &NodesUsageQuery) -> RemnaResult<NodesUsage> {
        self.read(|| self.inner.nodes_usage(query)).await
    }

    async fn bandwidth_stats(&self, tz: Option<&str>) -> RemnaResult<BandwidthStats> {
        self.read(|| self.inner.bandwidth_stats(tz)).await
    }

    async fn nodes_metrics(&self) -> RemnaResult<NodesMetrics> {
        self.read(|| self.inner.nodes_metrics()).await
    }

    async fn torrent_blocker_reports(
        &self,
        query: &TorrentReportsQuery,
    ) -> RemnaResult<TorrentReports> {
        self.read(|| self.inner.torrent_blocker_reports(query))
            .await
    }

    async fn torrent_blocker_stats(&self) -> RemnaResult<TorrentStats> {
        self.read(|| self.inner.torrent_blocker_stats()).await
    }

    async fn execute_node_plugin(&self, request: &PluginExecutorRequest) -> RemnaResult<()> {
        self.write(|| self.inner.execute_node_plugin(request)).await
    }

    async fn bulk_delete_users(&self, request: &BulkUsersRequest) -> RemnaResult<()> {
        self.write(|| self.inner.bulk_delete_users(request)).await
    }

    async fn bulk_revoke_subscriptions(&self, request: &BulkUsersRequest) -> RemnaResult<()> {
        self.write(|| self.inner.bulk_revoke_subscriptions(request))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeRemnawave, Fault, sample_user};
    use chrono::{NaiveDate, Utc};

    struct ManualClock {
        base: Instant,
        offset: Mutex<Duration>,
    }

    impl ManualClock {
        fn advance(&self, by: Duration) {
            let mut offset = self.offset.lock().unwrap();
            *offset += by;
        }
    }

    impl MonotonicClock for ManualClock {
        fn now(&self) -> Instant {
            self.base + *self.offset.lock().unwrap()
        }
    }

    fn guarded() -> (GuardedRemnawave<FakeRemnawave>, Arc<ManualClock>) {
        let clock = Arc::new(ManualClock {
            base: Instant::now(),
            offset: Mutex::new(Duration::ZERO),
        });
        let fake = FakeRemnawave::new();
        fake.insert_user(sample_user(1, "sn_a"));
        let guard = GuardedRemnawave::with_config(fake, BreakerConfig::default(), clock.clone());
        (guard, clock)
    }

    fn patch() -> UpdateUserRequest {
        let mut request = UpdateUserRequest::new(RemnaUserId(1));
        request.hwid_device_limit = Some(Some(3));
        request
    }

    fn is_open(err: &RemnaError) -> bool {
        matches!(err, RemnaError::Unavailable(message) if message == BREAKER_OPEN)
    }

    #[tokio::test]
    async fn три_сбоя_подряд_открывают_предохранитель_на_30_секунд() {
        let (guard, clock) = guarded();
        for _ in 0..3 {
            guard.inner().fail_on("update_user", Fault::ServerError);
            let err = guard.update_user(&patch()).await.unwrap_err();
            assert!(matches!(err, RemnaError::Server { .. }));
        }
        assert_eq!(guard.state(), BreakerState::Open);
        let err = guard.get_user(RemnaUserId(1)).await.unwrap_err();
        assert!(is_open(&err));
        assert!(err.outcome_unknown());
        assert_eq!(guard.inner().calls().len(), 3);
        clock.advance(Duration::from_secs(29));
        assert!(is_open(&guard.get_user(RemnaUserId(1)).await.unwrap_err()));
        clock.advance(Duration::from_secs(1));
        assert_eq!(guard.state(), BreakerState::HalfOpen);
        assert!(guard.get_user(RemnaUserId(1)).await.is_ok());
        assert_eq!(guard.state(), BreakerState::Closed);
        assert_eq!(guard.inner().calls().len(), 4);
    }

    #[tokio::test]
    async fn неудачная_проба_снова_открывает_предохранитель() {
        let (guard, clock) = guarded();
        for _ in 0..3 {
            guard.inner().fail_next(Fault::Timeout);
            assert_eq!(
                guard.get_user(RemnaUserId(1)).await,
                Err(RemnaError::Timeout)
            );
        }
        clock.advance(Duration::from_secs(30));
        guard.inner().fail_next(Fault::ServerError);
        assert!(matches!(
            guard.get_user(RemnaUserId(1)).await,
            Err(RemnaError::Server { .. })
        ));
        assert_eq!(guard.state(), BreakerState::Open);
        assert!(is_open(&guard.get_user(RemnaUserId(1)).await.unwrap_err()));
        clock.advance(Duration::from_secs(30));
        assert!(guard.get_user(RemnaUserId(1)).await.is_ok());
        assert_eq!(guard.state(), BreakerState::Closed);
    }

    #[tokio::test]
    async fn пока_идёт_проба_другие_запросы_получают_отказ() {
        let (guard, clock) = guarded();
        for _ in 0..3 {
            guard.settle(Verdict::Failure);
        }
        clock.advance(Duration::from_secs(30));
        assert!(guard.admit().is_ok());
        assert!(is_open(&guard.admit().unwrap_err()));
        clock.advance(Duration::from_secs(30));
        assert!(guard.admit().is_ok());
        guard.settle(Verdict::Neutral);
        assert!(guard.admit().is_ok());
        guard.settle(Verdict::Success);
        assert_eq!(guard.state(), BreakerState::Closed);
    }

    #[tokio::test]
    async fn ответы_401_и_429_это_сбои_а_404_нет() {
        let (guard, _clock) = guarded();
        guard.inner().fail_next(Fault::Unauthorized);
        assert_eq!(
            guard.update_user(&patch()).await,
            Err(RemnaError::Unauthorized(401))
        );
        guard.inner().fail_next(Fault::RateLimited);
        assert_eq!(
            guard.update_user(&patch()).await,
            Err(RemnaError::RateLimited)
        );
        assert!(
            guard
                .get_user(RemnaUserId(404))
                .await
                .unwrap_err()
                .is_not_found()
        );
        for _ in 0..2 {
            guard.inner().fail_next(Fault::ServerError);
            assert!(guard.update_user(&patch()).await.is_err());
        }
        assert_eq!(guard.state(), BreakerState::Closed);
        guard.inner().fail_next(Fault::Unauthorized);
        assert!(guard.update_user(&patch()).await.is_err());
        assert_eq!(guard.state(), BreakerState::Open);
    }

    #[tokio::test]
    async fn чтение_повторяется_один_раз_при_обрыве_а_изменение_нет() {
        let (guard, _clock) = guarded();
        guard.inner().fail_next(Fault::Unavailable);
        assert!(guard.get_user(RemnaUserId(1)).await.is_ok());
        assert_eq!(guard.inner().calls(), vec!["get_user", "get_user"]);
        guard.inner().fail_next(Fault::Unavailable);
        assert!(matches!(
            guard.update_user(&patch()).await,
            Err(RemnaError::Unavailable(_))
        ));
        assert_eq!(guard.inner().calls_of("update_user").len(), 1);
        guard.inner().fail_next(Fault::Timeout);
        assert_eq!(
            guard.get_user(RemnaUserId(1)).await,
            Err(RemnaError::Timeout)
        );
        assert_eq!(guard.inner().calls_of("get_user").len(), 3);
        guard.inner().fail_next(Fault::Unavailable);
        guard.inner().fail_next(Fault::Unavailable);
        assert!(guard.get_user(RemnaUserId(1)).await.is_err());
        assert_eq!(guard.state(), BreakerState::Open);
    }

    fn unsupported<T>(result: RemnaResult<T>) -> bool {
        matches!(result, Err(RemnaError::Unsupported(_)))
    }

    #[tokio::test]
    async fn предохранитель_пропускает_все_методы_панели() {
        let (g, _clock) = guarded();
        let id = RemnaUserId(1);
        let node = Uuid::nil();
        let day = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
        let bulk = BulkUsersRequest { user_ids: vec![id] };
        let profile = NodeProfileSelection {
            active_config_profile_uuid: Uuid::nil(),
            active_inbounds: Vec::new(),
        };
        let checks = vec![
            ("metadata", unsupported(g.metadata().await)),
            ("auth_status", unsupported(g.auth_status().await)),
            ("configuration", unsupported(g.configuration().await)),
            ("stats", unsupported(g.stats().await)),
            ("get_user", unsupported(g.get_user(id).await)),
            (
                "by_username",
                unsupported(g.get_user_by_username("sn_a").await),
            ),
            (
                "by_short",
                unsupported(g.get_user_by_short_uuid("short1").await),
            ),
            (
                "stream",
                unsupported(g.stream_users(&UsersStreamQuery::default()).await),
            ),
            ("update_user", unsupported(g.update_user(&patch()).await)),
            ("extend", unsupported(g.extend_user(id, 1).await)),
            ("reset", unsupported(g.reset_user_traffic(id).await)),
            ("disable", unsupported(g.disable_user(id).await)),
            ("enable", unsupported(g.enable_user(id).await)),
            (
                "revoke",
                unsupported(g.revoke_user(id, &RevokeRequest::default()).await),
            ),
            ("accessible", unsupported(g.user_accessible_nodes(id).await)),
            (
                "requests",
                unsupported(g.user_subscription_requests(id).await),
            ),
            (
                "usage",
                unsupported(
                    g.user_usage(
                        id,
                        DateRange {
                            start: day,
                            end: day,
                        },
                    )
                    .await,
                ),
            ),
            ("devices", unsupported(g.user_devices(id).await)),
            (
                "delete_device",
                unsupported(g.delete_user_device(id, "hw").await),
            ),
            (
                "delete_devices",
                unsupported(g.delete_all_user_devices(id).await),
            ),
            (
                "drop",
                unsupported(
                    g.drop_connections(&DropConnectionsRequest::users_everywhere(vec![id]))
                        .await,
                ),
            ),
            (
                "conn_job",
                unsupported(g.request_user_connections(id).await),
            ),
            ("conn", unsupported(g.user_connections("job-1").await)),
            (
                "bulk_update",
                unsupported(
                    g.bulk_update_users(&BulkUpdateRequest {
                        user_ids: vec![id],
                        fields: BulkUserFields::default(),
                    })
                    .await,
                ),
            ),
            (
                "bulk_squads",
                unsupported(
                    g.bulk_update_squads(&BulkSquadsRequest {
                        user_ids: vec![id],
                        active_internal_squads: Vec::new(),
                    })
                    .await,
                ),
            ),
            (
                "bulk_extend",
                unsupported(
                    g.bulk_extend_users(&BulkExtendRequest {
                        user_ids: vec![id],
                        extend_days: 1,
                    })
                    .await,
                ),
            ),
            ("bulk_reset", unsupported(g.bulk_reset_traffic(&bulk).await)),
            ("nodes", unsupported(g.nodes().await)),
            ("restart", unsupported(g.restart_node(node).await)),
            ("enable_node", unsupported(g.enable_node(node).await)),
            ("disable_node", unsupported(g.disable_node(node).await)),
            ("squads", unsupported(g.internal_squads().await)),
            (
                "squad_nodes",
                unsupported(g.squad_accessible_nodes(node).await),
            ),
            ("settings", unsupported(g.subscription_settings().await)),
            (
                "update_settings",
                unsupported(
                    g.update_subscription_settings(&UpdateSubscriptionSettingsRequest {
                        uuid: node,
                        is_show_custom_remarks: None,
                        custom_remarks: None,
                        hwid_settings: None,
                    })
                    .await,
                ),
            ),
            ("billing_nodes", unsupported(g.infra_billing_nodes().await)),
            ("node", unsupported(g.node(node).await)),
            ("nodes_detailed", unsupported(g.nodes_detailed().await)),
            (
                "create_node",
                unsupported(
                    g.create_node(&CreateNodeRequest::new("N", "198.51.100.1", profile))
                        .await,
                ),
            ),
            (
                "update_node",
                unsupported(g.update_node(&UpdateNodeRequest::new(node)).await),
            ),
            ("delete_node", unsupported(g.delete_node(node).await)),
            ("reset_node", unsupported(g.reset_node_traffic(node).await)),
            ("restart_all", unsupported(g.restart_all_nodes(false).await)),
            ("keygen", unsupported(g.keygen().await)),
            ("profiles", unsupported(g.config_profiles().await)),
            ("hosts", unsupported(g.hosts().await)),
            (
                "update_host",
                unsupported(g.update_host(&UpdateHostRequest::new(node)).await),
            ),
            ("enable_hosts", unsupported(g.enable_hosts(&[node]).await)),
            ("disable_hosts", unsupported(g.disable_hosts(&[node]).await)),
            (
                "create_host",
                unsupported(
                    g.create_host(&CreateHostRequest::new(
                        HostInboundTarget {
                            config_profile_uuid: node,
                            config_profile_inbound_uuid: node,
                        },
                        "H",
                        "198.51.100.1",
                        443,
                    ))
                    .await,
                ),
            ),
            ("delete_host", unsupported(g.delete_host(node).await)),
            ("reorder_hosts", unsupported(g.reorder_hosts(&[]).await)),
            (
                "create_squad",
                unsupported(
                    g.create_internal_squad(&CreateInternalSquadRequest {
                        name: "S".into(),
                        inbounds: Vec::new(),
                    })
                    .await,
                ),
            ),
            (
                "update_squad",
                unsupported(
                    g.update_internal_squad(&UpdateInternalSquadRequest {
                        uuid: node,
                        name: None,
                        inbounds: None,
                    })
                    .await,
                ),
            ),
            (
                "delete_squad",
                unsupported(g.delete_internal_squad(node).await),
            ),
            (
                "squad_usage",
                unsupported(
                    g.internal_squad_usage(
                        node,
                        &SquadUsageQuery {
                            start: day,
                            end: day,
                            min_total_bytes: None,
                            limit: None,
                            cursor: None,
                        },
                    )
                    .await,
                ),
            ),
            ("external", unsupported(g.external_squads().await)),
            ("providers", unsupported(g.infra_providers().await)),
            ("provider", unsupported(g.infra_provider(node).await)),
            (
                "create_provider",
                unsupported(
                    g.create_infra_provider(&CreateInfraProviderRequest {
                        name: "P".into(),
                        favicon_link: None,
                        login_url: None,
                    })
                    .await,
                ),
            ),
            (
                "update_provider",
                unsupported(
                    g.update_infra_provider(&UpdateInfraProviderRequest {
                        uuid: node,
                        name: None,
                        favicon_link: None,
                        login_url: None,
                    })
                    .await,
                ),
            ),
            (
                "delete_provider",
                unsupported(g.delete_infra_provider(node).await),
            ),
            (
                "history",
                unsupported(
                    g.infra_billing_history(&BillingHistoryQuery::default())
                        .await,
                ),
            ),
            (
                "create_record",
                unsupported(
                    g.create_infra_billing_record(&CreateBillingRecordRequest {
                        provider_uuid: node,
                        amount: BillingAmount(1),
                        billed_at: Utc::now(),
                    })
                    .await,
                ),
            ),
            (
                "create_billing_node",
                unsupported(
                    g.create_infra_billing_node(&CreateBillingNodeRequest {
                        provider_uuid: node,
                        node_uuid: None,
                        name: None,
                        next_billing_at: Utc::now(),
                    })
                    .await,
                ),
            ),
            (
                "update_billing_nodes",
                unsupported(
                    g.update_infra_billing_nodes(&UpdateBillingNodesRequest {
                        uuids: Vec::new(),
                        next_billing_at: Utc::now(),
                    })
                    .await,
                ),
            ),
            ("metadata_node", unsupported(g.node_metadata(node).await)),
            (
                "set_metadata",
                unsupported(g.set_node_metadata(node, &NodeMetadata::default()).await),
            ),
            (
                "page_configs",
                unsupported(g.subscription_page_configs().await),
            ),
            (
                "page_config",
                unsupported(g.subscription_page_config(node).await),
            ),
            ("subpage", unsupported(g.subpage_access("short1").await)),
            (
                "node_conn_job",
                unsupported(g.request_node_connections(node).await),
            ),
            ("node_conn", unsupported(g.node_connections("node-x").await)),
            (
                "nodes_usage",
                unsupported(
                    g.nodes_usage(&NodesUsageQuery {
                        start: day,
                        end: day,
                        top_nodes_limit: None,
                    })
                    .await,
                ),
            ),
            ("bandwidth", unsupported(g.bandwidth_stats(None).await)),
            ("metrics", unsupported(g.nodes_metrics().await)),
            (
                "torrent",
                unsupported(
                    g.torrent_blocker_reports(&TorrentReportsQuery::default())
                        .await,
                ),
            ),
            (
                "torrent_stats",
                unsupported(g.torrent_blocker_stats().await),
            ),
            (
                "executor",
                unsupported(
                    g.execute_node_plugin(&PluginExecutorRequest::unblock_everywhere(vec![
                        "203.0.113.7".into(),
                    ]))
                    .await,
                ),
            ),
            (
                "bulk_revoke",
                unsupported(g.bulk_revoke_subscriptions(&bulk).await),
            ),
            ("bulk_delete", unsupported(g.bulk_delete_users(&bulk).await)),
            ("delete_user", unsupported(g.delete_user(id).await)),
        ];
        let missing: Vec<&str> = checks
            .iter()
            .filter(|(_, unsupported)| *unsupported)
            .map(|(name, _)| *name)
            .collect();
        assert!(missing.is_empty(), "не проброшены: {missing:?}");
        assert_eq!(checks.len(), 82);
        let created = g
            .create_user(&CreateUserRequest {
                username: "sn_new".into(),
                status: None,
                expire_at: Utc::now(),
                traffic_limit_bytes: 0,
                traffic_limit_strategy: sn_domain::traffic::TrafficStrategy::NoReset,
                description: None,
                tag: None,
                telegram_id: None,
                email: None,
                hwid_device_limit: None,
                active_internal_squads: Vec::new(),
                external_squad_uuid: None,
            })
            .await;
        assert!(created.is_ok());
        assert_eq!(g.state(), BreakerState::Closed);
    }
}
