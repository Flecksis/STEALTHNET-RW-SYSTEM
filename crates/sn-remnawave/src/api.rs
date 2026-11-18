use crate::error::{RemnaError, RemnaResult};
use crate::models::*;
use async_trait::async_trait;
use sn_domain::ids::RemnaUserId;
use uuid::Uuid;

#[async_trait]
pub trait RemnawaveApi: Send + Sync {
    async fn metadata(&self) -> RemnaResult<SystemMetadata>;
    async fn configuration(&self) -> RemnaResult<SystemConfiguration>;
    async fn stats(&self) -> RemnaResult<SystemStats>;

    async fn auth_status(&self) -> RemnaResult<()> {
        Err(RemnaError::Unsupported("auth.status"))
    }

    async fn get_user(&self, id: RemnaUserId) -> RemnaResult<RemnaUser>;
    async fn get_user_by_username(&self, username: &str) -> RemnaResult<RemnaUser>;
    async fn get_user_by_short_uuid(&self, short_uuid: &str) -> RemnaResult<RemnaUser>;
    async fn stream_users(&self, query: &UsersStreamQuery) -> RemnaResult<UsersPage>;
    async fn create_user(&self, request: &CreateUserRequest) -> RemnaResult<RemnaUser>;
    async fn update_user(&self, request: &UpdateUserRequest) -> RemnaResult<RemnaUser>;
    async fn delete_user(&self, id: RemnaUserId) -> RemnaResult<()>;

    async fn extend_user(&self, id: RemnaUserId, days: u32) -> RemnaResult<RemnaUser>;
    async fn reset_user_traffic(&self, id: RemnaUserId) -> RemnaResult<RemnaUser>;
    async fn disable_user(&self, id: RemnaUserId) -> RemnaResult<RemnaUser>;
    async fn enable_user(&self, id: RemnaUserId) -> RemnaResult<RemnaUser>;
    async fn revoke_user(&self, id: RemnaUserId, request: &RevokeRequest)
    -> RemnaResult<RemnaUser>;

    async fn user_accessible_nodes(&self, id: RemnaUserId) -> RemnaResult<UserAccessibleNodes>;
    async fn user_subscription_requests(
        &self,
        id: RemnaUserId,
    ) -> RemnaResult<SubscriptionRequests>;
    async fn user_usage(&self, id: RemnaUserId, range: DateRange) -> RemnaResult<UserUsage>;

    async fn user_devices(&self, id: RemnaUserId) -> RemnaResult<HwidDevices>;
    async fn delete_user_device(&self, id: RemnaUserId, hwid: &str) -> RemnaResult<HwidDevices>;
    async fn delete_all_user_devices(&self, id: RemnaUserId) -> RemnaResult<HwidDevices>;

    async fn drop_connections(&self, request: &DropConnectionsRequest) -> RemnaResult<()>;
    async fn request_user_connections(&self, id: RemnaUserId) -> RemnaResult<ConnectionsJob>;
    async fn user_connections(&self, job_id: &str) -> RemnaResult<UserConnectionsStatus>;

    async fn bulk_update_users(&self, request: &BulkUpdateRequest) -> RemnaResult<()>;
    async fn bulk_update_squads(&self, request: &BulkSquadsRequest) -> RemnaResult<()>;
    async fn bulk_extend_users(&self, request: &BulkExtendRequest) -> RemnaResult<()>;
    async fn bulk_reset_traffic(&self, request: &BulkUsersRequest) -> RemnaResult<()>;

    async fn nodes(&self) -> RemnaResult<Vec<Node>>;
    async fn restart_node(&self, uuid: Uuid) -> RemnaResult<()>;
    async fn enable_node(&self, uuid: Uuid) -> RemnaResult<Node>;
    async fn disable_node(&self, uuid: Uuid) -> RemnaResult<Node>;

    async fn internal_squads(&self) -> RemnaResult<InternalSquads>;
    async fn squad_accessible_nodes(&self, uuid: Uuid) -> RemnaResult<SquadAccessibleNodes>;

    async fn subscription_settings(&self) -> RemnaResult<SubscriptionSettings>;
    async fn update_subscription_settings(
        &self,
        request: &UpdateSubscriptionSettingsRequest,
    ) -> RemnaResult<SubscriptionSettings>;

    async fn infra_billing_nodes(&self) -> RemnaResult<InfraBillingNodes>;

    async fn node(&self, _uuid: Uuid) -> RemnaResult<NodeDetails> {
        Err(RemnaError::Unsupported("nodes.get"))
    }

    async fn nodes_detailed(&self) -> RemnaResult<Vec<NodeDetails>> {
        Err(RemnaError::Unsupported("nodes.list"))
    }

    async fn create_node(&self, _request: &CreateNodeRequest) -> RemnaResult<NodeDetails> {
        Err(RemnaError::Unsupported("nodes.create"))
    }

    async fn update_node(&self, _request: &UpdateNodeRequest) -> RemnaResult<NodeDetails> {
        Err(RemnaError::Unsupported("nodes.update"))
    }

    async fn delete_node(&self, _uuid: Uuid) -> RemnaResult<()> {
        Err(RemnaError::Unsupported("nodes.delete"))
    }

    async fn reset_node_traffic(&self, _uuid: Uuid) -> RemnaResult<()> {
        Err(RemnaError::Unsupported("nodes.reset_traffic"))
    }

    async fn restart_all_nodes(&self, _force: bool) -> RemnaResult<()> {
        Err(RemnaError::Unsupported("nodes.restart_all"))
    }

    async fn keygen(&self) -> RemnaResult<NodeKeygen> {
        Err(RemnaError::Unsupported("keygen"))
    }

    async fn config_profiles(&self) -> RemnaResult<ConfigProfiles> {
        Err(RemnaError::Unsupported("config_profiles"))
    }

    async fn hosts(&self) -> RemnaResult<Vec<Host>> {
        Err(RemnaError::Unsupported("hosts.list"))
    }

    async fn update_host(&self, _request: &UpdateHostRequest) -> RemnaResult<Host> {
        Err(RemnaError::Unsupported("hosts.update"))
    }

    async fn create_host(&self, _request: &CreateHostRequest) -> RemnaResult<Host> {
        Err(RemnaError::Unsupported("hosts.create"))
    }

    async fn delete_host(&self, _uuid: Uuid) -> RemnaResult<()> {
        Err(RemnaError::Unsupported("hosts.delete"))
    }

    async fn reorder_hosts(&self, _positions: &[HostPosition]) -> RemnaResult<()> {
        Err(RemnaError::Unsupported("hosts.reorder"))
    }

    async fn enable_hosts(&self, _uuids: &[Uuid]) -> RemnaResult<()> {
        Err(RemnaError::Unsupported("hosts.bulk_enable"))
    }

    async fn disable_hosts(&self, _uuids: &[Uuid]) -> RemnaResult<()> {
        Err(RemnaError::Unsupported("hosts.bulk_disable"))
    }

    async fn create_internal_squad(
        &self,
        _request: &CreateInternalSquadRequest,
    ) -> RemnaResult<InternalSquad> {
        Err(RemnaError::Unsupported("internal_squads.create"))
    }

    async fn update_internal_squad(
        &self,
        _request: &UpdateInternalSquadRequest,
    ) -> RemnaResult<InternalSquad> {
        Err(RemnaError::Unsupported("internal_squads.update"))
    }

    async fn delete_internal_squad(&self, _uuid: Uuid) -> RemnaResult<()> {
        Err(RemnaError::Unsupported("internal_squads.delete"))
    }

    async fn internal_squad_usage(
        &self,
        _uuid: Uuid,
        _query: &SquadUsageQuery,
    ) -> RemnaResult<SquadUsage> {
        Err(RemnaError::Unsupported("internal_squads.usage"))
    }

    async fn external_squads(&self) -> RemnaResult<ExternalSquads> {
        Err(RemnaError::Unsupported("external_squads.list"))
    }

    async fn infra_providers(&self) -> RemnaResult<InfraProviders> {
        Err(RemnaError::Unsupported("infra_billing.providers"))
    }

    async fn infra_provider(&self, _uuid: Uuid) -> RemnaResult<InfraProvider> {
        Err(RemnaError::Unsupported("infra_billing.provider"))
    }

    async fn create_infra_provider(
        &self,
        _request: &CreateInfraProviderRequest,
    ) -> RemnaResult<InfraProvider> {
        Err(RemnaError::Unsupported("infra_billing.create_provider"))
    }

    async fn update_infra_provider(
        &self,
        _request: &UpdateInfraProviderRequest,
    ) -> RemnaResult<InfraProvider> {
        Err(RemnaError::Unsupported("infra_billing.update_provider"))
    }

    async fn delete_infra_provider(&self, _uuid: Uuid) -> RemnaResult<()> {
        Err(RemnaError::Unsupported("infra_billing.delete_provider"))
    }

    async fn infra_billing_history(
        &self,
        _query: &BillingHistoryQuery,
    ) -> RemnaResult<InfraBillingHistory> {
        Err(RemnaError::Unsupported("infra_billing.history"))
    }

    async fn create_infra_billing_record(
        &self,
        _request: &CreateBillingRecordRequest,
    ) -> RemnaResult<InfraBillingHistory> {
        Err(RemnaError::Unsupported("infra_billing.create_record"))
    }

    async fn create_infra_billing_node(
        &self,
        _request: &CreateBillingNodeRequest,
    ) -> RemnaResult<InfraBillingNodes> {
        Err(RemnaError::Unsupported("infra_billing.create_node"))
    }

    async fn update_infra_billing_nodes(
        &self,
        _request: &UpdateBillingNodesRequest,
    ) -> RemnaResult<InfraBillingNodes> {
        Err(RemnaError::Unsupported("infra_billing.update_nodes"))
    }

    async fn node_metadata(&self, _uuid: Uuid) -> RemnaResult<NodeMetadata> {
        Err(RemnaError::Unsupported("metadata.node"))
    }

    async fn set_node_metadata(
        &self,
        _uuid: Uuid,
        _metadata: &NodeMetadata,
    ) -> RemnaResult<NodeMetadata> {
        Err(RemnaError::Unsupported("metadata.set_node"))
    }

    async fn subscription_page_configs(&self) -> RemnaResult<SubpageConfigs> {
        Err(RemnaError::Unsupported("subscription_page_configs"))
    }

    async fn subpage_access(&self, _short_uuid: &str) -> RemnaResult<SubpageAccess> {
        Err(RemnaError::Unsupported("subscriptions.subpage_config"))
    }

    async fn request_node_connections(&self, _uuid: Uuid) -> RemnaResult<ConnectionsJob> {
        Err(RemnaError::Unsupported("connections.by_node"))
    }

    async fn node_connections(&self, _job_id: &str) -> RemnaResult<NodeConnectionsStatus> {
        Err(RemnaError::Unsupported("connections.by_node_result"))
    }

    async fn nodes_usage(&self, _query: &NodesUsageQuery) -> RemnaResult<NodesUsage> {
        Err(RemnaError::Unsupported("bandwidth_stats.nodes"))
    }

    async fn bandwidth_stats(&self, _tz: Option<&str>) -> RemnaResult<BandwidthStats> {
        Err(RemnaError::Unsupported("system.stats_bandwidth"))
    }

    async fn nodes_metrics(&self) -> RemnaResult<NodesMetrics> {
        Err(RemnaError::Unsupported("system.nodes_metrics"))
    }

    async fn torrent_blocker_reports(
        &self,
        _query: &TorrentReportsQuery,
    ) -> RemnaResult<TorrentReports> {
        Err(RemnaError::Unsupported("torrent_blocker.reports"))
    }

    async fn torrent_blocker_stats(&self) -> RemnaResult<TorrentStats> {
        Err(RemnaError::Unsupported("torrent_blocker.stats"))
    }

    async fn execute_node_plugin(&self, _request: &PluginExecutorRequest) -> RemnaResult<()> {
        Err(RemnaError::Unsupported("node_plugins.executor"))
    }

    async fn bulk_delete_users(&self, _request: &BulkUsersRequest) -> RemnaResult<()> {
        Err(RemnaError::Unsupported("users.bulk_delete"))
    }

    async fn bulk_revoke_subscriptions(&self, _request: &BulkUsersRequest) -> RemnaResult<()> {
        Err(RemnaError::Unsupported("users.bulk_revoke"))
    }
}
