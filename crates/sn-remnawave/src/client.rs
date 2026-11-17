use crate::api::RemnawaveApi;
use crate::error::{RemnaError, RemnaResult};
use crate::models::*;
use async_trait::async_trait;
use reqwest::header::{ACCEPT, COOKIE};
use reqwest::{Method, RequestBuilder};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sn_domain::ids::RemnaUserId;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RemnaConfig {
    pub base_url: String,
    pub token: String,
    pub cookie: Option<String>,
    pub forwarded_headers: Option<bool>,
    pub timeout: Duration,
    pub connect_timeout: Duration,
}

impl RemnaConfig {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        RemnaConfig {
            base_url: base_url.into(),
            token: token.into(),
            cookie: None,
            forwarded_headers: None,
            timeout: Duration::from_secs(12),
            connect_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RemnawaveClient {
    http: reqwest::Client,
    base: String,
    token: String,
    cookie: Option<String>,
    forwarded: bool,
}

#[derive(Deserialize)]
struct Envelope<T> {
    response: T,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RestartNodeBody {
    force_restart: bool,
}

pub fn normalize_cookie(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.contains('=') {
        return Some(raw.to_string());
    }
    raw.split_once(':')
        .map(|(name, value)| format!("{}={}", name.trim(), value.trim()))
}

fn segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn transport(err: reqwest::Error) -> RemnaError {
    let err = err.without_url();
    if err.is_timeout() {
        RemnaError::Timeout
    } else if err.is_decode() {
        RemnaError::Decode(err.to_string())
    } else {
        RemnaError::Unavailable(err.to_string())
    }
}

impl RemnawaveClient {
    pub fn new(config: RemnaConfig) -> RemnaResult<Self> {
        let base = config.base_url.trim().trim_end_matches('/').to_string();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(RemnaError::NotConfigured(
                "адрес панели должен начинаться с http:// или https://".into(),
            ));
        }
        if config.token.trim().is_empty() {
            return Err(RemnaError::NotConfigured("не задан API-токен".into()));
        }
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .connect_timeout(config.connect_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("stealthnet/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| RemnaError::NotConfigured(e.to_string()))?;
        let forwarded = config
            .forwarded_headers
            .unwrap_or_else(|| base.starts_with("http://"));
        Ok(RemnawaveClient {
            http,
            base,
            token: config.token.trim().to_string(),
            cookie: config.cookie.as_deref().and_then(normalize_cookie),
            forwarded,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let mut builder = self
            .http
            .request(method, format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
            .header(ACCEPT, "application/json");
        if let Some(cookie) = &self.cookie {
            builder = builder.header(COOKIE, cookie);
        }
        if self.forwarded {
            builder = builder
                .header("x-forwarded-for", "127.0.0.1")
                .header("x-forwarded-proto", "https");
        }
        builder
    }

    async fn send(&self, builder: RequestBuilder) -> RemnaResult<String> {
        let response = builder.send().await.map_err(transport)?;
        let status = response.status();
        let body = response.text().await.map_err(transport)?;
        if status.is_success() {
            Ok(body)
        } else {
            Err(RemnaError::from_status(status.as_u16(), &body))
        }
    }

    async fn call<T: DeserializeOwned>(&self, builder: RequestBuilder) -> RemnaResult<T> {
        let body = self.send(builder).await?;
        serde_json::from_str::<Envelope<T>>(&body)
            .map(|e| e.response)
            .map_err(|e| RemnaError::Decode(e.to_string()))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> RemnaResult<T> {
        self.call(self.request(Method::GET, path)).await
    }

    async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> RemnaResult<T> {
        self.call(self.request(Method::POST, path).json(body)).await
    }

    async fn post_empty<T: DeserializeOwned>(&self, path: &str) -> RemnaResult<T> {
        self.call(self.request(Method::POST, path)).await
    }

    async fn post_accepted<B: Serialize + ?Sized>(&self, path: &str, body: &B) -> RemnaResult<()> {
        self.send(self.request(Method::POST, path).json(body))
            .await
            .map(|_| ())
    }

    async fn post_no_body(&self, path: &str) -> RemnaResult<()> {
        self.send(self.request(Method::POST, path))
            .await
            .map(|_| ())
    }

    async fn patch<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> RemnaResult<T> {
        self.call(self.request(Method::PATCH, path).json(body))
            .await
    }

    async fn put<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> RemnaResult<T> {
        self.call(self.request(Method::PUT, path).json(body)).await
    }

    async fn delete(&self, path: &str) -> RemnaResult<()> {
        self.send(self.request(Method::DELETE, path))
            .await
            .map(|_| ())
    }

    async fn get_query<Q: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        query: &Q,
    ) -> RemnaResult<T> {
        self.call(self.request(Method::GET, path).query(query))
            .await
    }
}

#[derive(Serialize)]
struct HostUuids<'a> {
    uuids: &'a [Uuid],
}

#[derive(Serialize)]
struct TimezoneQuery<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    tz: Option<&'a str>,
}

#[async_trait]
impl RemnawaveApi for RemnawaveClient {
    async fn metadata(&self) -> RemnaResult<SystemMetadata> {
        self.get("/api/system/metadata").await
    }

    async fn configuration(&self) -> RemnaResult<SystemConfiguration> {
        self.get("/api/system/configuration").await
    }

    async fn stats(&self) -> RemnaResult<SystemStats> {
        self.get("/api/system/stats").await
    }

    async fn get_user(&self, id: RemnaUserId) -> RemnaResult<RemnaUser> {
        self.get(&format!("/api/users/{id}")).await
    }

    async fn get_user_by_username(&self, username: &str) -> RemnaResult<RemnaUser> {
        self.get(&format!("/api/users/by-username/{}", segment(username)))
            .await
    }

    async fn get_user_by_short_uuid(&self, short_uuid: &str) -> RemnaResult<RemnaUser> {
        self.get(&format!("/api/users/by-short-uuid/{}", segment(short_uuid)))
            .await
    }

    async fn stream_users(&self, query: &UsersStreamQuery) -> RemnaResult<UsersPage> {
        self.call(self.request(Method::GET, "/api/users/stream").query(query))
            .await
    }

    async fn create_user(&self, request: &CreateUserRequest) -> RemnaResult<RemnaUser> {
        self.post("/api/users", request).await
    }

    async fn update_user(&self, request: &UpdateUserRequest) -> RemnaResult<RemnaUser> {
        self.call(self.request(Method::PATCH, "/api/users").json(request))
            .await
    }

    async fn delete_user(&self, id: RemnaUserId) -> RemnaResult<()> {
        self.send(self.request(Method::DELETE, &format!("/api/users/{id}")))
            .await
            .map(|_| ())
    }

    async fn extend_user(&self, id: RemnaUserId, days: u32) -> RemnaResult<RemnaUser> {
        self.post(
            &format!("/api/users/{id}/actions/extend"),
            &ExtendRequest { days },
        )
        .await
    }

    async fn reset_user_traffic(&self, id: RemnaUserId) -> RemnaResult<RemnaUser> {
        self.post_empty(&format!("/api/users/{id}/actions/reset-traffic"))
            .await
    }

    async fn disable_user(&self, id: RemnaUserId) -> RemnaResult<RemnaUser> {
        self.post_empty(&format!("/api/users/{id}/actions/disable"))
            .await
    }

    async fn enable_user(&self, id: RemnaUserId) -> RemnaResult<RemnaUser> {
        self.post_empty(&format!("/api/users/{id}/actions/enable"))
            .await
    }

    async fn revoke_user(
        &self,
        id: RemnaUserId,
        request: &RevokeRequest,
    ) -> RemnaResult<RemnaUser> {
        self.post(&format!("/api/users/{id}/actions/revoke"), request)
            .await
    }

    async fn user_accessible_nodes(&self, id: RemnaUserId) -> RemnaResult<UserAccessibleNodes> {
        self.get(&format!("/api/users/{id}/accessible-nodes")).await
    }

    async fn user_subscription_requests(
        &self,
        id: RemnaUserId,
    ) -> RemnaResult<SubscriptionRequests> {
        self.get(&format!("/api/users/{id}/subscription-request-history"))
            .await
    }

    async fn user_usage(&self, id: RemnaUserId, range: DateRange) -> RemnaResult<UserUsage> {
        self.call(
            self.request(Method::GET, &format!("/api/bandwidth-stats/users/{id}"))
                .query(&range),
        )
        .await
    }

    async fn user_devices(&self, id: RemnaUserId) -> RemnaResult<HwidDevices> {
        self.get(&format!("/api/hwid/devices/{id}")).await
    }

    async fn delete_user_device(&self, id: RemnaUserId, hwid: &str) -> RemnaResult<HwidDevices> {
        self.post(
            "/api/hwid/devices/delete",
            &DeleteDeviceRequest {
                user_id: id,
                hwid: hwid.to_string(),
            },
        )
        .await
    }

    async fn delete_all_user_devices(&self, id: RemnaUserId) -> RemnaResult<HwidDevices> {
        self.post(
            "/api/hwid/devices/delete-all",
            &DeleteAllDevicesRequest { user_id: id },
        )
        .await
    }

    async fn drop_connections(&self, request: &DropConnectionsRequest) -> RemnaResult<()> {
        self.post_accepted("/api/connections/drop", request).await
    }

    async fn request_user_connections(&self, id: RemnaUserId) -> RemnaResult<ConnectionsJob> {
        self.post_empty(&format!("/api/connections/by-user/{id}"))
            .await
    }

    async fn user_connections(&self, job_id: &str) -> RemnaResult<UserConnectionsStatus> {
        self.get(&format!("/api/connections/by-user/{}", segment(job_id)))
            .await
    }

    async fn bulk_update_users(&self, request: &BulkUpdateRequest) -> RemnaResult<()> {
        self.post_accepted("/api/users/bulk/update", request).await
    }

    async fn bulk_update_squads(&self, request: &BulkSquadsRequest) -> RemnaResult<()> {
        self.post_accepted("/api/users/bulk/update-squads", request)
            .await
    }

    async fn bulk_extend_users(&self, request: &BulkExtendRequest) -> RemnaResult<()> {
        self.post_accepted("/api/users/bulk/extend-expiration-date", request)
            .await
    }

    async fn bulk_reset_traffic(&self, request: &BulkUsersRequest) -> RemnaResult<()> {
        self.post_accepted("/api/users/bulk/reset-traffic", request)
            .await
    }

    async fn nodes(&self) -> RemnaResult<Vec<Node>> {
        self.get("/api/nodes").await
    }

    async fn restart_node(&self, uuid: Uuid) -> RemnaResult<()> {
        self.post_accepted(
            &format!("/api/nodes/{uuid}/actions/restart"),
            &RestartNodeBody {
                force_restart: false,
            },
        )
        .await
    }

    async fn enable_node(&self, uuid: Uuid) -> RemnaResult<Node> {
        self.post_empty(&format!("/api/nodes/{uuid}/actions/enable"))
            .await
    }

    async fn disable_node(&self, uuid: Uuid) -> RemnaResult<Node> {
        self.post_empty(&format!("/api/nodes/{uuid}/actions/disable"))
            .await
    }

    async fn internal_squads(&self) -> RemnaResult<InternalSquads> {
        self.get("/api/internal-squads").await
    }

    async fn squad_accessible_nodes(&self, uuid: Uuid) -> RemnaResult<SquadAccessibleNodes> {
        self.get(&format!("/api/internal-squads/{uuid}/accessible-nodes"))
            .await
    }

    async fn subscription_settings(&self) -> RemnaResult<SubscriptionSettings> {
        self.get("/api/subscription-settings").await
    }

    async fn update_subscription_settings(
        &self,
        request: &UpdateSubscriptionSettingsRequest,
    ) -> RemnaResult<SubscriptionSettings> {
        self.call(
            self.request(Method::PATCH, "/api/subscription-settings")
                .json(request),
        )
        .await
    }

    async fn infra_billing_nodes(&self) -> RemnaResult<InfraBillingNodes> {
        self.get("/api/infra-billing/nodes").await
    }

    async fn node(&self, uuid: Uuid) -> RemnaResult<NodeDetails> {
        self.get(&format!("/api/nodes/{uuid}")).await
    }

    async fn nodes_detailed(&self) -> RemnaResult<Vec<NodeDetails>> {
        self.get("/api/nodes").await
    }

    async fn create_node(&self, request: &CreateNodeRequest) -> RemnaResult<NodeDetails> {
        self.post("/api/nodes", request).await
    }

    async fn update_node(&self, request: &UpdateNodeRequest) -> RemnaResult<NodeDetails> {
        self.patch("/api/nodes", request).await
    }

    async fn delete_node(&self, uuid: Uuid) -> RemnaResult<()> {
        self.delete(&format!("/api/nodes/{uuid}")).await
    }

    async fn reset_node_traffic(&self, uuid: Uuid) -> RemnaResult<()> {
        self.post_no_body(&format!("/api/nodes/{uuid}/actions/reset-traffic"))
            .await
    }

    async fn restart_all_nodes(&self, force: bool) -> RemnaResult<()> {
        self.post_accepted(
            "/api/nodes/actions/restart-all",
            &RestartNodeBody {
                force_restart: force,
            },
        )
        .await
    }

    async fn keygen(&self) -> RemnaResult<NodeKeygen> {
        self.get("/api/keygen").await
    }

    async fn config_profiles(&self) -> RemnaResult<ConfigProfiles> {
        self.get("/api/config-profiles").await
    }

    async fn hosts(&self) -> RemnaResult<Vec<Host>> {
        self.get("/api/hosts").await
    }

    async fn update_host(&self, request: &UpdateHostRequest) -> RemnaResult<Host> {
        self.patch("/api/hosts", request).await
    }

    async fn enable_hosts(&self, uuids: &[Uuid]) -> RemnaResult<()> {
        self.post_accepted("/api/hosts/bulk/enable", &HostUuids { uuids })
            .await
    }

    async fn disable_hosts(&self, uuids: &[Uuid]) -> RemnaResult<()> {
        self.post_accepted("/api/hosts/bulk/disable", &HostUuids { uuids })
            .await
    }

    async fn create_internal_squad(
        &self,
        request: &CreateInternalSquadRequest,
    ) -> RemnaResult<InternalSquad> {
        self.post("/api/internal-squads", request).await
    }

    async fn update_internal_squad(
        &self,
        request: &UpdateInternalSquadRequest,
    ) -> RemnaResult<InternalSquad> {
        self.patch("/api/internal-squads", request).await
    }

    async fn delete_internal_squad(&self, uuid: Uuid) -> RemnaResult<()> {
        self.delete(&format!("/api/internal-squads/{uuid}")).await
    }

    async fn internal_squad_usage(
        &self,
        uuid: Uuid,
        query: &SquadUsageQuery,
    ) -> RemnaResult<SquadUsage> {
        self.get_query(&format!("/api/internal-squads/{uuid}/usage"), query)
            .await
    }

    async fn external_squads(&self) -> RemnaResult<ExternalSquads> {
        self.get("/api/external-squads").await
    }

    async fn infra_providers(&self) -> RemnaResult<InfraProviders> {
        self.get("/api/infra-billing/providers").await
    }

    async fn infra_provider(&self, uuid: Uuid) -> RemnaResult<InfraProvider> {
        self.get(&format!("/api/infra-billing/providers/{uuid}"))
            .await
    }

    async fn create_infra_provider(
        &self,
        request: &CreateInfraProviderRequest,
    ) -> RemnaResult<InfraProvider> {
        self.post("/api/infra-billing/providers", request).await
    }

    async fn update_infra_provider(
        &self,
        request: &UpdateInfraProviderRequest,
    ) -> RemnaResult<InfraProvider> {
        self.patch("/api/infra-billing/providers", request).await
    }

    async fn delete_infra_provider(&self, uuid: Uuid) -> RemnaResult<()> {
        self.delete(&format!("/api/infra-billing/providers/{uuid}"))
            .await
    }

    async fn infra_billing_history(
        &self,
        query: &BillingHistoryQuery,
    ) -> RemnaResult<InfraBillingHistory> {
        self.get_query("/api/infra-billing/history", query).await
    }

    async fn create_infra_billing_record(
        &self,
        request: &CreateBillingRecordRequest,
    ) -> RemnaResult<InfraBillingHistory> {
        self.post("/api/infra-billing/history", request).await
    }

    async fn create_infra_billing_node(
        &self,
        request: &CreateBillingNodeRequest,
    ) -> RemnaResult<InfraBillingNodes> {
        self.post("/api/infra-billing/nodes", request).await
    }

    async fn update_infra_billing_nodes(
        &self,
        request: &UpdateBillingNodesRequest,
    ) -> RemnaResult<InfraBillingNodes> {
        self.patch("/api/infra-billing/nodes", request).await
    }

    async fn node_metadata(&self, uuid: Uuid) -> RemnaResult<NodeMetadata> {
        self.get(&format!("/api/metadata/node/{uuid}")).await
    }

    async fn set_node_metadata(
        &self,
        uuid: Uuid,
        metadata: &NodeMetadata,
    ) -> RemnaResult<NodeMetadata> {
        self.put(&format!("/api/metadata/node/{uuid}"), metadata)
            .await
    }

    async fn subscription_page_configs(&self) -> RemnaResult<SubpageConfigs> {
        self.get("/api/subscription-page-configs").await
    }

    async fn subpage_access(&self, short_uuid: &str) -> RemnaResult<SubpageAccess> {
        self.get(&format!(
            "/api/subscriptions/subpage-config/{}",
            segment(short_uuid)
        ))
        .await
    }

    async fn request_node_connections(&self, uuid: Uuid) -> RemnaResult<ConnectionsJob> {
        self.post_empty(&format!("/api/connections/by-node/{uuid}"))
            .await
    }

    async fn node_connections(&self, job_id: &str) -> RemnaResult<NodeConnectionsStatus> {
        self.get(&format!("/api/connections/by-node/{}", segment(job_id)))
            .await
    }

    async fn nodes_usage(&self, query: &NodesUsageQuery) -> RemnaResult<NodesUsage> {
        self.get_query("/api/bandwidth-stats/nodes", query).await
    }

    async fn bandwidth_stats(&self, tz: Option<&str>) -> RemnaResult<BandwidthStats> {
        self.get_query("/api/system/stats/bandwidth", &TimezoneQuery { tz })
            .await
    }

    async fn nodes_metrics(&self) -> RemnaResult<NodesMetrics> {
        self.get("/api/system/nodes/metrics").await
    }

    async fn torrent_blocker_reports(
        &self,
        query: &TorrentReportsQuery,
    ) -> RemnaResult<TorrentReports> {
        self.get_query("/api/node-plugins/torrent-blocker", query)
            .await
    }

    async fn torrent_blocker_stats(&self) -> RemnaResult<TorrentStats> {
        self.get("/api/node-plugins/torrent-blocker/stats").await
    }

    async fn execute_node_plugin(&self, request: &PluginExecutorRequest) -> RemnaResult<()> {
        self.post_accepted("/api/node-plugins/executor", request)
            .await
    }

    async fn bulk_delete_users(&self, request: &BulkUsersRequest) -> RemnaResult<()> {
        self.post_accepted("/api/users/bulk/delete", request).await
    }

    async fn bulk_revoke_subscriptions(&self, request: &BulkUsersRequest) -> RemnaResult<()> {
        self.post_accepted("/api/users/bulk/revoke-subscription", request)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, NaiveDate, Utc};
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn user_json(id: i64) -> serde_json::Value {
        serde_json::json!({
            "id": id, "shortUuid": "abc", "username": "sn_x", "status": "ACTIVE",
            "trafficLimitBytes": 0, "trafficLimitStrategy": "NO_RESET",
            "expireAt": "2027-02-14T00:00:00.000Z", "createdAt": "2026-11-16T14:52:00.000Z",
            "updatedAt": "2026-11-16T14:52:00.000Z", "subscriptionUrl": "https://sub.example.com/abc",
            "activeInternalSquads": [{"uuid": "00000000-0000-0000-0000-000000000001", "name": "Основной"}],
            "userTraffic": {"usedTrafficBytes": 0, "lifetimeUsedTrafficBytes": 0}
        })
    }

    fn client(server: &MockServer) -> RemnawaveClient {
        let mut config = RemnaConfig::new(server.uri(), "token-123");
        config.cookie = Some("guard:secret".into());
        RemnawaveClient::new(config).unwrap()
    }

    #[test]
    fn кука_в_обоих_форматах() {
        assert_eq!(
            normalize_cookie("name:value").as_deref(),
            Some("name=value")
        );
        assert_eq!(
            normalize_cookie("name=value").as_deref(),
            Some("name=value")
        );
        assert_eq!(normalize_cookie("  "), None);
    }

    #[test]
    fn адрес_без_схемы_не_принимается() {
        assert!(RemnawaveClient::new(RemnaConfig::new("panel.example.com", "t")).is_err());
        assert!(RemnawaveClient::new(RemnaConfig::new("https://panel.example.com/", "")).is_err());
    }

    #[tokio::test]
    async fn пользователь_читается_из_обёртки_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/users/42"))
            .and(header("authorization", "Bearer token-123"))
            .and(header("cookie", "guard=secret"))
            .and(header("x-forwarded-proto", "https"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"response": user_json(42)})),
            )
            .mount(&server)
            .await;
        let user = client(&server).get_user(RemnaUserId(42)).await.unwrap();
        assert_eq!(user.id, RemnaUserId(42));
        assert_eq!(user.squad_ids().len(), 1);
    }

    #[tokio::test]
    async fn не_найден_это_not_found_а_не_сбой() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/users/7"))
            .respond_with(ResponseTemplate::new(404).set_body_json(
                serde_json::json!({"message": "User not found", "errorCode": "A063"}),
            ))
            .mount(&server)
            .await;
        let err = client(&server).get_user(RemnaUserId(7)).await.unwrap_err();
        assert!(err.is_not_found());
        assert_eq!(err.code(), Some("A063"));
    }

    #[tokio::test]
    async fn продление_отправляет_дни() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/users/42/actions/extend"))
            .and(body_json(serde_json::json!({"days": 30})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"response": user_json(42)})),
            )
            .mount(&server)
            .await;
        assert!(
            client(&server)
                .extend_user(RemnaUserId(42), 30)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn массовые_операции_принимают_202_без_тела() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/users/bulk/reset-traffic"))
            .respond_with(ResponseTemplate::new(202))
            .mount(&server)
            .await;
        let request = BulkUsersRequest {
            user_ids: vec![RemnaUserId(1), RemnaUserId(2)],
        };
        assert!(client(&server).bulk_reset_traffic(&request).await.is_ok());
    }

    const NODE: &str = "00000000-0000-4000-8000-000000000011";
    const PROFILE: &str = "00000000-0000-4000-8000-000000000021";
    const INBOUND: &str = "00000000-0000-4000-8000-000000000031";
    const SQUAD: &str = "00000000-0000-4000-8000-000000000041";
    const PROVIDER: &str = "00000000-0000-4000-8000-000000000051";
    const BILLING: &str = "00000000-0000-4000-8000-000000000061";
    const HOST: &str = "00000000-0000-4000-8000-000000000071";

    fn uuid(raw: &str) -> Uuid {
        Uuid::parse_str(raw).unwrap()
    }

    fn wrapped(value: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({ "response": value }))
    }

    fn inbound_json() -> serde_json::Value {
        serde_json::json!({
            "uuid": INBOUND, "profileUuid": PROFILE, "tag": "VLESS_REALITY", "type": "vless",
            "network": "tcp", "security": "reality", "port": 443, "rawInbound": {"privateKey": "секрет"}
        })
    }

    fn node_json() -> serde_json::Value {
        serde_json::json!({
            "uuid": NODE, "id": 3, "name": "NL-2", "address": "198.51.100.22", "port": 2222,
            "isConnected": false, "isDisabled": false, "isConnecting": false,
            "lastStatusChange": "2026-11-16T14:38:00.000Z", "lastStatusMessage": "connect ETIMEDOUT",
            "isTrafficTrackingActive": true, "trafficResetDay": 1, "trafficLimitBytes": 100,
            "trafficUsedBytes": 92, "notifyPercent": 90, "viewPosition": 2, "countryCode": "NL",
            "consumptionMultiplier": 1, "nodeConsumptionMultiplier": 1, "tags": [], "usersOnline": 0,
            "xrayUptime": 0, "providerUuid": PROVIDER,
            "provider": {"uuid": PROVIDER, "name": "Aeza", "faviconLink": null, "loginUrl": "https://aeza.example"},
            "configProfile": {"activeConfigProfileUuid": PROFILE, "activeInbounds": [inbound_json()]},
            "system": {"info": {"arch": "x64", "cpus": 2, "cpuModel": "EPYC", "memoryTotal": 4294967296_i64,
                       "hostname": "nl-2", "platform": "linux", "release": "6.8.0", "type": "Linux",
                       "version": "#47-Ubuntu", "networkInterfaces": ["eth0"]},
                       "stats": {"memoryFree": 1, "memoryUsed": 2254857830_i64, "uptime": 1036800.5,
                                 "loadAvg": [0.76, 0.5, 0.4], "interface": null}},
            "versions": {"xray": "25.10.15", "node": "2.3.1"},
            "createdAt": "2026-10-01T00:00:00.000Z", "updatedAt": "2026-11-16T14:38:00.000Z"
        })
    }

    fn squad_json() -> serde_json::Value {
        serde_json::json!({
            "uuid": SQUAD, "viewPosition": 0, "name": "Резерв", "tags": [],
            "info": {"membersCount": 1980, "inboundsCount": 1}, "inbounds": [inbound_json()],
            "createdAt": "2026-10-01T00:00:00.000Z", "updatedAt": "2026-10-01T00:00:00.000Z"
        })
    }

    fn provider_json() -> serde_json::Value {
        serde_json::json!({
            "uuid": PROVIDER, "name": "Aeza", "faviconLink": null, "loginUrl": "https://aeza.example",
            "createdAt": "2026-10-01T00:00:00.000Z", "updatedAt": "2026-10-01T00:00:00.000Z",
            "billingHistory": {"totalAmount": 1342.6, "totalBills": 12},
            "billingNodes": [{"name": "NL-1", "details": {"nodeUuid": NODE, "countryCode": "NL"}}]
        })
    }

    fn billing_nodes_json() -> serde_json::Value {
        serde_json::json!({
            "totalBillingNodes": 1,
            "billingNodes": [{
                "uuid": BILLING, "nodeUuid": NODE, "name": null, "providerUuid": PROVIDER,
                "provider": {"uuid": PROVIDER, "name": "Aeza", "loginUrl": null, "faviconLink": null},
                "node": {"uuid": NODE, "name": "NL-2", "countryCode": "NL"},
                "nextBillingAt": "2026-12-02T00:00:00.000Z",
                "createdAt": "2026-10-01T00:00:00.000Z", "updatedAt": "2026-10-01T00:00:00.000Z"
            }],
            "availableBillingNodes": [], "totalAvailableBillingNodes": 0,
            "stats": {"upcomingNodesCount": 1, "currentMonthPayments": 80.09, "totalSpent": 1342.6}
        })
    }

    fn history_json() -> serde_json::Value {
        serde_json::json!({
            "records": [{
                "uuid": BILLING, "providerUuid": PROVIDER, "amount": 7.59,
                "billedAt": "2026-11-16T14:52:00.000Z",
                "provider": {"uuid": PROVIDER, "name": "Hetzner", "faviconLink": null}
            }],
            "total": 1
        })
    }

    fn day(raw: &str) -> NaiveDate {
        NaiveDate::parse_from_str(raw, "%Y-%m-%d").unwrap()
    }

    fn at(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[tokio::test]
    async fn нода_создаётся_меняется_читается_и_удаляется() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/nodes"))
            .and(body_json(serde_json::json!({
                "name": "NL-2", "address": "198.51.100.22", "port": 2222, "countryCode": "NL",
                "configProfile": {"activeConfigProfileUuid": PROFILE, "activeInbounds": [INBOUND]},
                "providerUuid": PROVIDER
            })))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({ "response": node_json() })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/api/nodes"))
            .and(body_json(
                serde_json::json!({"uuid": NODE, "name": "NL-3", "note": null}),
            ))
            .respond_with(wrapped(node_json()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/nodes/{NODE}")))
            .respond_with(wrapped(node_json()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/nodes"))
            .respond_with(wrapped(serde_json::json!([node_json()])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("/api/nodes/{NODE}")))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let api = client(&server);
        let mut create = CreateNodeRequest::new(
            "NL-2",
            "198.51.100.22",
            NodeProfileSelection {
                active_config_profile_uuid: uuid(PROFILE),
                active_inbounds: vec![uuid(INBOUND)],
            },
        );
        create.port = Some(2222);
        create.country_code = Some("NL".into());
        create.provider_uuid = Some(uuid(PROVIDER));
        let created = api.create_node(&create).await.unwrap();
        assert_eq!(created.node.name, "NL-2");
        assert_eq!(created.node.state(), NodeState::Down);
        assert_eq!(created.provider_uuid, Some(uuid(PROVIDER)));
        let system = created.system.clone().unwrap();
        assert_eq!(system.info.cpus, 2);
        assert_eq!(system.info.os_type, "Linux");
        assert_eq!(system.stats.memory_used, 2_254_857_830);
        let profile = created.config_profile.clone().unwrap();
        assert_eq!(profile.active_inbounds[0].port, Some(443));
        let serialized = serde_json::to_string(&created).unwrap();
        assert!(!serialized.contains("секрет"));
        assert!(serialized.contains("\"isConnected\":false"));

        let mut update = UpdateNodeRequest::new(uuid(NODE));
        update.name = Some("NL-3".into());
        update.note = Some(None);
        assert_eq!(
            api.update_node(&update).await.unwrap().node.port,
            Some(2222)
        );
        assert_eq!(api.node(uuid(NODE)).await.unwrap(), created);
        assert_eq!(api.nodes_detailed().await.unwrap(), vec![created]);
        api.delete_node(uuid(NODE)).await.unwrap();
    }

    #[tokio::test]
    async fn сброс_трафика_ноды_перезапуск_всех_и_ключ() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/api/nodes/{NODE}/actions/reset-traffic")))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/nodes/actions/restart-all"))
            .and(body_json(serde_json::json!({"forceRestart": false})))
            .respond_with(ResponseTemplate::new(202))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/keygen"))
            .respond_with(wrapped(
                serde_json::json!({"secretKey": "eyJub2RlQ2VydFBlbSI6"}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let api = client(&server);
        api.reset_node_traffic(uuid(NODE)).await.unwrap();
        api.restart_all_nodes(false).await.unwrap();
        assert_eq!(
            api.keygen().await.unwrap().secret_key,
            "eyJub2RlQ2VydFBlbSI6"
        );
    }

    #[tokio::test]
    async fn профили_конфигурации_и_хосты() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/config-profiles"))
            .respond_with(wrapped(serde_json::json!({
                "total": 1,
                "configProfiles": [{
                    "uuid": PROFILE, "viewPosition": 0, "name": "Основной профиль", "tags": [],
                    "config": {"inbounds": [{"settings": {"privateKey": "секрет"}}]},
                    "inbounds": [inbound_json()],
                    "nodes": [{"uuid": NODE, "name": "NL-2", "countryCode": "NL"}],
                    "createdAt": "2026-10-01T00:00:00.000Z", "updatedAt": "2026-10-01T00:00:00.000Z"
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let host = serde_json::json!({
            "uuid": HOST, "viewPosition": 0, "remark": "Нидерланды, резерв", "address": "nl.example.com",
            "port": 443, "isDisabled": false, "isHidden": false, "shuffleHost": false, "mihomoX25519": false,
            "inbound": {"configProfileUuid": PROFILE, "configProfileInboundUuid": INBOUND},
            "nodes": [NODE], "tags": [], "serverDescription": null,
            "internalSquads": {"mode": "ALLOW_ONLY", "squads": [SQUAD]}
        });
        Mock::given(method("GET"))
            .and(path("/api/hosts"))
            .respond_with(wrapped(serde_json::json!([host])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/api/hosts"))
            .and(body_json(
                serde_json::json!({"uuid": HOST, "remark": "Нидерланды", "isHidden": true}),
            ))
            .respond_with(wrapped(host.clone()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/hosts/bulk/enable"))
            .and(body_json(serde_json::json!({"uuids": [HOST]})))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/hosts/bulk/disable"))
            .and(body_json(serde_json::json!({"uuids": [HOST]})))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let api = client(&server);
        let profiles = api.config_profiles().await.unwrap();
        assert_eq!(profiles.config_profiles[0].name, "Основной профиль");
        assert_eq!(profiles.config_profiles[0].inbounds[0].kind, "vless");
        assert_eq!(profiles.config_profiles[0].nodes[0].country_code, "NL");
        assert!(!serde_json::to_string(&profiles).unwrap().contains("секрет"));
        let hosts = api.hosts().await.unwrap();
        assert_eq!(hosts[0].remark, "Нидерланды, резерв");
        assert_eq!(hosts[0].nodes, vec![uuid(NODE)]);
        assert_eq!(
            hosts[0].internal_squads.as_ref().unwrap().squads,
            vec![uuid(SQUAD)]
        );
        let mut update = UpdateHostRequest::new(uuid(HOST));
        update.remark = Some("Нидерланды".into());
        update.is_hidden = Some(true);
        assert_eq!(api.update_host(&update).await.unwrap().uuid, uuid(HOST));
        api.enable_hosts(&[uuid(HOST)]).await.unwrap();
        api.disable_hosts(&[uuid(HOST)]).await.unwrap();
    }

    #[tokio::test]
    async fn сквады_создаются_меняются_удаляются_и_считают_трафик() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/internal-squads"))
            .and(body_json(
                serde_json::json!({"name": "Резерв", "inbounds": [INBOUND]}),
            ))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({ "response": squad_json() })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/api/internal-squads"))
            .and(body_json(
                serde_json::json!({"uuid": SQUAD, "inbounds": []}),
            ))
            .respond_with(wrapped(squad_json()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("/api/internal-squads/{SQUAD}")))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/internal-squads/{SQUAD}/usage")))
            .and(query_param("start", "2026-10-17"))
            .and(query_param("end", "2026-11-16"))
            .and(query_param("limit", "100"))
            .respond_with(wrapped(serde_json::json!({
                "squadUuid": SQUAD, "users": [{"id": 42, "totalBytes": 5368709120_i64}],
                "nextCursor": "43", "hasMore": true
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/external-squads"))
            .respond_with(wrapped(serde_json::json!({
                "total": 1,
                "externalSquads": [{
                    "uuid": SQUAD, "viewPosition": 0, "name": "Белые списки", "tags": [],
                    "info": {"membersCount": 312}, "templates": [],
                    "subscriptionSettings": null, "hostOverrides": null,
                    "responseHeadersAdd": {}, "responseHeadersRemove": [],
                    "hwidSettings": {"enabled": true, "fallbackDeviceLimit": 3, "maxDevicesAnnounce": null},
                    "customRemarks": null, "subpageConfigUuid": null,
                    "createdAt": "2026-10-01T00:00:00.000Z", "updatedAt": "2026-10-01T00:00:00.000Z"
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let api = client(&server);
        let created = api
            .create_internal_squad(&CreateInternalSquadRequest {
                name: "Резерв".into(),
                inbounds: vec![uuid(INBOUND)],
            })
            .await
            .unwrap();
        assert_eq!(created.info.members_count, 1980);
        assert_eq!(created.inbounds[0].tag, "VLESS_REALITY");
        let updated = api
            .update_internal_squad(&UpdateInternalSquadRequest {
                uuid: uuid(SQUAD),
                name: None,
                inbounds: Some(Vec::new()),
            })
            .await
            .unwrap();
        assert_eq!(updated.uuid, uuid(SQUAD));
        api.delete_internal_squad(uuid(SQUAD)).await.unwrap();
        let usage = api
            .internal_squad_usage(
                uuid(SQUAD),
                &SquadUsageQuery {
                    start: day("2026-10-17"),
                    end: day("2026-11-16"),
                    min_total_bytes: None,
                    limit: Some(100),
                    cursor: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(usage.users[0].id, RemnaUserId(42));
        assert_eq!(usage.users[0].total_bytes, 5_368_709_120);
        assert!(usage.has_more);
        let external = api.external_squads().await.unwrap();
        assert_eq!(external.external_squads[0].info.members_count, 312);
        assert_eq!(
            external.external_squads[0]
                .hwid_settings
                .as_ref()
                .unwrap()
                .fallback_device_limit,
            3
        );
    }

    #[tokio::test]
    async fn провайдеры_и_оплата_серверов() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/infra-billing/providers"))
            .respond_with(wrapped(
                serde_json::json!({"total": 1, "providers": [provider_json()]}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/infra-billing/providers/{PROVIDER}")))
            .respond_with(wrapped(provider_json()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/infra-billing/providers"))
            .and(body_json(
                serde_json::json!({"name": "Aeza", "loginUrl": "https://aeza.example"}),
            ))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({ "response": provider_json() })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/api/infra-billing/providers"))
            .and(body_json(
                serde_json::json!({"uuid": PROVIDER, "loginUrl": null}),
            ))
            .respond_with(wrapped(provider_json()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("/api/infra-billing/providers/{PROVIDER}")))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/infra-billing/history"))
            .and(query_param("start", "0"))
            .and(query_param("size", "5"))
            .respond_with(wrapped(history_json()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/infra-billing/history"))
            .and(body_json(serde_json::json!({
                "providerUuid": PROVIDER, "amount": 7.59, "billedAt": "2026-11-16T14:52:00Z"
            })))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({ "response": history_json() })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/infra-billing/nodes"))
            .and(body_json(serde_json::json!({
                "providerUuid": PROVIDER, "nodeUuid": NODE, "name": null,
                "nextBillingAt": "2026-12-02T00:00:00Z"
            })))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({ "response": billing_nodes_json() })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/api/infra-billing/nodes"))
            .and(body_json(serde_json::json!({
                "uuids": [BILLING], "nextBillingAt": "2026-12-18T00:00:00Z"
            })))
            .respond_with(wrapped(billing_nodes_json()))
            .expect(1)
            .mount(&server)
            .await;
        let api = client(&server);
        let providers = api.infra_providers().await.unwrap();
        assert_eq!(providers.providers[0].name, "Aeza");
        assert_eq!(
            providers.providers[0].billing_history.total_amount,
            BillingAmount(134_260)
        );
        assert_eq!(
            providers.providers[0].billing_nodes[0]
                .details
                .as_ref()
                .unwrap()
                .node_uuid,
            uuid(NODE)
        );
        assert_eq!(
            api.infra_provider(uuid(PROVIDER))
                .await
                .unwrap()
                .login_url
                .as_deref(),
            Some("https://aeza.example")
        );
        let created = api
            .create_infra_provider(&CreateInfraProviderRequest {
                name: "Aeza".into(),
                favicon_link: None,
                login_url: Some("https://aeza.example".into()),
            })
            .await
            .unwrap();
        assert_eq!(created.uuid, uuid(PROVIDER));
        api.update_infra_provider(&UpdateInfraProviderRequest {
            uuid: uuid(PROVIDER),
            name: None,
            favicon_link: None,
            login_url: Some(None),
        })
        .await
        .unwrap();
        api.delete_infra_provider(uuid(PROVIDER)).await.unwrap();
        let history = api
            .infra_billing_history(&BillingHistoryQuery {
                start: Some(0),
                size: Some(5),
            })
            .await
            .unwrap();
        assert_eq!(history.records[0].amount, BillingAmount(759));
        assert_eq!(
            history.records[0]
                .amount
                .money(sn_domain::money::Currency::Usd),
            sn_domain::money::Money::new(759, sn_domain::money::Currency::Usd)
        );
        let recorded = api
            .create_infra_billing_record(&CreateBillingRecordRequest {
                provider_uuid: uuid(PROVIDER),
                amount: BillingAmount(759),
                billed_at: at("2026-11-16T14:52:00Z"),
            })
            .await
            .unwrap();
        assert_eq!(recorded.total, 1);
        let billing = api
            .create_infra_billing_node(&CreateBillingNodeRequest {
                provider_uuid: uuid(PROVIDER),
                node_uuid: Some(uuid(NODE)),
                name: None,
                next_billing_at: at("2026-12-02T00:00:00Z"),
            })
            .await
            .unwrap();
        assert_eq!(billing.billing_nodes[0].provider.name, "Aeza");
        let moved = api
            .update_infra_billing_nodes(&UpdateBillingNodesRequest {
                uuids: vec![uuid(BILLING)],
                next_billing_at: at("2026-12-18T00:00:00Z"),
            })
            .await
            .unwrap();
        assert_eq!(moved.total_billing_nodes, 1);
    }

    #[tokio::test]
    async fn город_ноды_хранится_в_метаданных() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/metadata/node/{NODE}")))
            .respond_with(wrapped(
                serde_json::json!({"metadata": {"city": "Амстердам", "rack": 7}}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(format!("/api/metadata/node/{NODE}")))
            .and(body_json(
                serde_json::json!({"metadata": {"city": "Роттердам", "rack": 7}}),
            ))
            .respond_with(wrapped(
                serde_json::json!({"metadata": {"city": "Роттердам", "rack": 7}}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let api = client(&server);
        let mut metadata = api.node_metadata(uuid(NODE)).await.unwrap();
        assert_eq!(metadata.city(), Some("Амстердам"));
        metadata.set_city(Some(" Роттердам "));
        let saved = api.set_node_metadata(uuid(NODE), &metadata).await.unwrap();
        assert_eq!(saved.city(), Some("Роттердам"));
        metadata.set_city(None);
        assert_eq!(metadata.city(), None);
        assert!(metadata.metadata.contains_key("rack"));
    }

    #[tokio::test]
    async fn страница_подписки_и_её_доступность() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/subscription-page-configs"))
            .respond_with(wrapped(serde_json::json!({
                "total": 2,
                "configs": [
                    {"uuid": PROFILE, "viewPosition": 0, "name": "Основное", "tags": [], "config": {"x": 1}},
                    {"uuid": SQUAD, "viewPosition": 1, "name": "Для белых списков", "tags": [], "config": null}
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/subscriptions/subpage-config/abc%2Fdef"))
            .respond_with(wrapped(
                serde_json::json!({"subpageConfigUuid": PROFILE, "webpageAllowed": true}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let api = client(&server);
        let configs = api.subscription_page_configs().await.unwrap();
        let names: Vec<&str> = configs.configs.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Основное", "Для белых списков"]);
        let access = api.subpage_access("abc/def").await.unwrap();
        assert!(access.webpage_allowed);
        assert_eq!(access.subpage_config_uuid, Some(uuid(PROFILE)));
    }

    #[tokio::test]
    async fn подключения_ноды_и_статистика_трафика() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/api/connections/by-node/{NODE}")))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"response": {"jobId": "job-7"}})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/connections/by-node/job-7"))
            .respond_with(wrapped(serde_json::json!({
                "isCompleted": true, "isFailed": false,
                "result": {"success": true, "nodeUuid": NODE,
                           "users": [{"userId": 42, "ips": [{"ip": "203.0.113.7", "lastSeen": "2026-11-16T14:47:00.000Z"}]}]}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/bandwidth-stats/nodes"))
            .and(query_param("start", "2026-11-01"))
            .and(query_param("end", "2026-11-16"))
            .and(query_param("topNodesLimit", "6"))
            .respond_with(wrapped(serde_json::json!({
                "categories": ["2026-11-01"], "sparklineData": [6815744000000_i64],
                "topNodes": [{"uuid": NODE, "color": "#000", "name": "NL-2", "countryCode": "NL", "total": 6815744000000_i64}],
                "series": [{"uuid": NODE, "name": "NL-2", "color": "#000", "countryCode": "NL", "total": 1, "data": [1]}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/system/stats/bandwidth"))
            .and(query_param("tz", "Europe/Moscow"))
            .respond_with(wrapped(serde_json::json!({
                "bandwidthLastTwoDays": {"current": "1", "previous": "2", "difference": "-1"},
                "bandwidthLastSevenDays": {"current": "1", "previous": "2", "difference": "-1"},
                "bandwidthLast30Days": {"current": "34.0 TB", "previous": "30 TB", "difference": "4 TB"},
                "bandwidthCalendarMonth": {"current": "1", "previous": "2", "difference": "-1"},
                "bandwidthCurrentYear": {"current": "1", "previous": "2", "difference": "-1"}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/system/nodes/metrics"))
            .respond_with(wrapped(serde_json::json!({"nodes": [{
                "nodeUuid": NODE, "nodeName": "NL-2", "countryEmoji": "NL", "providerName": "Aeza",
                "usersOnline": 412,
                "inboundsStats": [{"tag": "VLESS_REALITY", "upload": "1", "download": "2"}],
                "outboundsStats": []
            }]})))
            .expect(1)
            .mount(&server)
            .await;
        let api = client(&server);
        let job = api.request_node_connections(uuid(NODE)).await.unwrap();
        let status = api.node_connections(&job.job_id).await.unwrap();
        let result = status.result.unwrap();
        assert_eq!(result.users[0].user_id, RemnaUserId(42));
        assert_eq!(result.users[0].ips[0].ip, "203.0.113.7");
        let usage = api
            .nodes_usage(&NodesUsageQuery {
                start: day("2026-11-01"),
                end: day("2026-11-16"),
                top_nodes_limit: Some(6),
            })
            .await
            .unwrap();
        assert_eq!(usage.top_nodes[0].name, "NL-2");
        assert_eq!(usage.series[0].data, vec![1.0]);
        let bandwidth = api.bandwidth_stats(Some("Europe/Moscow")).await.unwrap();
        assert_eq!(bandwidth.last_30_days.current, "34.0 TB");
        let metrics = api.nodes_metrics().await.unwrap();
        assert_eq!(metrics.nodes[0].users_online, 412);
        assert_eq!(metrics.nodes[0].inbounds_stats[0].tag, "VLESS_REALITY");
    }

    #[tokio::test]
    async fn торрент_блокер_без_адресов_назначения() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/node-plugins/torrent-blocker"))
            .and(query_param("size", "25"))
            .respond_with(wrapped(serde_json::json!({
                "records": [{
                    "id": 1, "userId": 42, "nodeId": 3, "user": {"username": "sn_x"},
                    "node": {"uuid": NODE, "name": "NL-2", "countryCode": "NL"},
                    "report": {
                        "actionReport": {"blocked": true, "ip": "203.0.113.7", "blockDuration": 3600,
                                         "willUnblockAt": "2026-11-16T15:52:00.000Z", "userId": "42",
                                         "processedAt": "2026-11-16T14:52:00.000Z"},
                        "xrayReport": {"destination": "tracker.example.org:6969", "network": "tcp", "ts": 1}
                    },
                    "createdAt": "2026-11-16T14:52:00.000Z"
                }],
                "total": 1
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/node-plugins/torrent-blocker/stats"))
            .respond_with(wrapped(serde_json::json!({
                "stats": {"distinctNodes": 1, "distinctUsers": 1, "totalReports": 3, "reportsLast24Hours": 2},
                "topUsers": [{"userId": 42, "color": "#000", "username": "sn_x", "total": 3}],
                "topNodes": [{"uuid": NODE, "countryCode": "NL", "color": "#000", "name": "NL-2", "total": 3}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/node-plugins/executor"))
            .and(body_json(serde_json::json!({
                "command": {"command": "unblockIps", "ips": ["203.0.113.7"]},
                "targetNodes": {"target": "allNodes"}
            })))
            .respond_with(ResponseTemplate::new(202))
            .expect(1)
            .mount(&server)
            .await;
        let api = client(&server);
        let reports = api
            .torrent_blocker_reports(&TorrentReportsQuery {
                start: None,
                size: Some(25),
            })
            .await
            .unwrap();
        let report = &reports.records[0];
        assert_eq!(report.user_id, RemnaUserId(42));
        assert_eq!(report.report.action_report.ip, "203.0.113.7");
        assert_eq!(
            report.report.action_report.will_unblock_at,
            Some(at("2026-11-16T15:52:00Z"))
        );
        assert!(
            !serde_json::to_string(&reports)
                .unwrap()
                .contains("tracker.example.org")
        );
        let stats = api.torrent_blocker_stats().await.unwrap();
        assert_eq!(stats.stats.reports_last_24_hours, 2);
        assert_eq!(stats.top_users[0].username, "sn_x");
        api.execute_node_plugin(&PluginExecutorRequest::unblock_everywhere(vec![
            "203.0.113.7".into(),
        ]))
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn массовое_удаление_и_перевыпуск_подписок() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/users/bulk/delete"))
            .and(body_json(serde_json::json!({"userIds": [1, 2]})))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/users/bulk/revoke-subscription"))
            .and(body_json(serde_json::json!({"userIds": [3]})))
            .respond_with(ResponseTemplate::new(202))
            .expect(1)
            .mount(&server)
            .await;
        let api = client(&server);
        api.bulk_delete_users(&BulkUsersRequest {
            user_ids: vec![RemnaUserId(1), RemnaUserId(2)],
        })
        .await
        .unwrap();
        api.bulk_revoke_subscriptions(&BulkUsersRequest {
            user_ids: vec![RemnaUserId(3)],
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn ошибка_сервера_значит_исход_неизвестен() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/api/users"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream down"))
            .mount(&server)
            .await;
        let err = client(&server)
            .update_user(&UpdateUserRequest::new(RemnaUserId(1)))
            .await
            .unwrap_err();
        assert!(err.outcome_unknown());
    }
}
