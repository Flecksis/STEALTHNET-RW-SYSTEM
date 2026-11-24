use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sn_domain::ids::{RemnaUserId, SquadId};
use sn_domain::money::{Currency, Money, PayCurrency, minor_to_units};
use sn_domain::plan::RemnaUserState;
use sn_domain::subscription::{RemnaMirror, SubscriptionStatus};
use sn_domain::traffic::TrafficStrategy;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserStatus {
    Active,
    Disabled,
    Limited,
    Expired,
}

impl From<UserStatus> for SubscriptionStatus {
    fn from(value: UserStatus) -> Self {
        match value {
            UserStatus::Active => SubscriptionStatus::Active,
            UserStatus::Disabled => SubscriptionStatus::Disabled,
            UserStatus::Limited => SubscriptionStatus::Limited,
            UserStatus::Expired => SubscriptionStatus::Expired,
        }
    }
}

impl From<SubscriptionStatus> for UserStatus {
    fn from(value: SubscriptionStatus) -> Self {
        match value {
            SubscriptionStatus::Active => UserStatus::Active,
            SubscriptionStatus::Disabled => UserStatus::Disabled,
            SubscriptionStatus::Limited => UserStatus::Limited,
            SubscriptionStatus::Expired => UserStatus::Expired,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadRef {
    pub uuid: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserTraffic {
    #[serde(default, deserialize_with = "nullable")]
    pub used_traffic_bytes: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub lifetime_used_traffic_bytes: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub online_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "nullable")]
    pub first_connected_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "nullable")]
    pub last_connected_node_uuid: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemnaUser {
    pub id: RemnaUserId,
    pub short_uuid: String,
    pub username: String,
    pub status: UserStatus,
    pub traffic_limit_bytes: i64,
    pub traffic_limit_strategy: TrafficStrategy,
    pub expire_at: DateTime<Utc>,
    #[serde(default, deserialize_with = "nullable")]
    pub telegram_id: Option<i64>,
    #[serde(default, deserialize_with = "nullable")]
    pub email: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub tag: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub hwid_device_limit: Option<i64>,
    #[serde(default, deserialize_with = "nullable")]
    pub external_squad_uuid: Option<Uuid>,
    #[serde(default, deserialize_with = "nullable")]
    pub sub_revoked_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "nullable")]
    pub last_traffic_reset_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, deserialize_with = "nullable")]
    pub subscription_url: String,
    #[serde(default, deserialize_with = "nullable")]
    pub active_internal_squads: Vec<SquadRef>,
    #[serde(default, deserialize_with = "nullable")]
    pub user_traffic: UserTraffic,
    #[serde(default, deserialize_with = "nullable", skip_serializing)]
    pub vless_uuid: Option<Secret>,
    #[serde(default, deserialize_with = "nullable", skip_serializing)]
    pub trojan_password: Option<Secret>,
    #[serde(default, deserialize_with = "nullable", skip_serializing)]
    pub ss_password: Option<Secret>,
}

#[derive(Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UserKeys {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vless_uuid: Option<Secret>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trojan_password: Option<Secret>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ss_password: Option<Secret>,
}

pub const PASSWORD_MIN: usize = 8;
pub const PASSWORD_MAX: usize = 32;

fn password_fits(value: &Secret) -> bool {
    (PASSWORD_MIN..=PASSWORD_MAX).contains(&value.expose().chars().count())
}

impl UserKeys {
    pub fn is_empty(&self) -> bool {
        self.vless_uuid.is_none() && self.trojan_password.is_none() && self.ss_password.is_none()
    }

    pub fn usable(&self) -> UserKeys {
        UserKeys {
            vless_uuid: self
                .vless_uuid
                .clone()
                .filter(|v| Uuid::parse_str(v.expose()).is_ok()),
            trojan_password: self.trojan_password.clone().filter(password_fits),
            ss_password: self.ss_password.clone().filter(password_fits),
        }
    }
}

fn to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

impl RemnaUser {
    pub fn squad_ids(&self) -> Vec<SquadId> {
        self.active_internal_squads
            .iter()
            .map(|s| SquadId(s.uuid))
            .collect()
    }

    pub fn state(&self) -> RemnaUserState {
        RemnaUserState {
            user_id: self.id,
            status: self.status.into(),
            expire_at: self.expire_at,
            squads: self.squad_ids(),
            traffic_limit_bytes: to_u64(self.traffic_limit_bytes),
            traffic_used_bytes: to_u64(self.user_traffic.used_traffic_bytes),
            traffic_strategy: self.traffic_limit_strategy,
            hwid_device_limit: self.hwid_device_limit.and_then(|v| u32::try_from(v).ok()),
        }
    }

    pub fn keys(&self) -> UserKeys {
        UserKeys {
            vless_uuid: self.vless_uuid.clone(),
            trojan_password: self.trojan_password.clone(),
            ss_password: self.ss_password.clone(),
        }
    }

    pub fn mirror(&self, devices_count: Option<u32>, synced_at: DateTime<Utc>) -> RemnaMirror {
        RemnaMirror {
            status: Some(self.status.into()),
            expire_at: Some(self.expire_at),
            traffic_used_bytes: Some(to_u64(self.user_traffic.used_traffic_bytes)),
            traffic_limit_bytes: Some(to_u64(self.traffic_limit_bytes)),
            traffic_strategy: Some(self.traffic_limit_strategy),
            hwid_limit: self.hwid_device_limit.and_then(|v| u32::try_from(v).ok()),
            devices_count,
            online_at: self.user_traffic.online_at,
            first_connected_at: self.user_traffic.first_connected_at,
            last_node: self.user_traffic.last_connected_node_uuid,
            remna_updated_at: Some(self.updated_at),
            synced_at: Some(synced_at),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserRequest {
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<UserStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vless_uuid: Option<Secret>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trojan_password: Option<Secret>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ss_password: Option<Secret>,
    pub expire_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_traffic_reset_at: Option<DateTime<Utc>>,
    pub traffic_limit_bytes: i64,
    pub traffic_limit_strategy: TrafficStrategy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hwid_device_limit: Option<i64>,
    pub active_internal_squads: Vec<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_squad_uuid: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserRequest {
    pub id: RemnaUserId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<UserStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_limit_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_limit_strategy: Option<TrafficStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expire_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram_id: Option<Option<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hwid_device_limit: Option<Option<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_internal_squads: Option<Vec<Uuid>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_squad_uuid: Option<Option<Uuid>>,
}

impl UpdateUserRequest {
    pub const fn new(id: RemnaUserId) -> Self {
        UpdateUserRequest {
            id,
            status: None,
            traffic_limit_bytes: None,
            traffic_limit_strategy: None,
            expire_at: None,
            description: None,
            tag: None,
            telegram_id: None,
            email: None,
            hwid_device_limit: None,
            active_internal_squads: None,
            external_squad_uuid: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExtendRequest {
    pub days: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoke_only_passwords: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_uuid: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsersStreamQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<UserStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsersPage {
    pub users: Vec<RemnaUser>,
    #[serde(default, deserialize_with = "nullable")]
    pub next_cursor: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HwidDevice {
    pub hwid: String,
    pub user_id: RemnaUserId,
    #[serde(default, deserialize_with = "nullable")]
    pub platform: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub os_version: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub device_model: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub user_agent: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub request_ip: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HwidDevices {
    pub total: i64,
    pub devices: Vec<HwidDevice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteDeviceRequest {
    pub user_id: RemnaUserId,
    pub hwid: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteAllDevicesRequest {
    pub user_id: RemnaUserId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "by")]
pub enum DropBy {
    #[serde(rename = "userIds")]
    UserIds {
        #[serde(rename = "userIds")]
        user_ids: Vec<RemnaUserId>,
    },
    #[serde(rename = "ipAddresses")]
    IpAddresses {
        #[serde(rename = "ipAddresses")]
        ip_addresses: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "target")]
pub enum TargetNodes {
    #[serde(rename = "allNodes")]
    AllNodes,
    #[serde(rename = "specificNodes")]
    SpecificNodes {
        #[serde(rename = "nodeUuids")]
        node_uuids: Vec<Uuid>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DropConnectionsRequest {
    pub drop_by: DropBy,
    pub target_nodes: TargetNodes,
}

impl DropConnectionsRequest {
    pub fn users_everywhere(user_ids: Vec<RemnaUserId>) -> Self {
        DropConnectionsRequest {
            drop_by: DropBy::UserIds { user_ids },
            target_nodes: TargetNodes::AllNodes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkUsersRequest {
    pub user_ids: Vec<RemnaUserId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkUserFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<UserStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_limit_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_limit_strategy: Option<TrafficStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expire_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hwid_device_limit: Option<Option<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkUpdateRequest {
    pub user_ids: Vec<RemnaUserId>,
    pub fields: BulkUserFields,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkSquadsRequest {
    pub user_ids: Vec<RemnaUserId>,
    pub active_internal_squads: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkExtendRequest {
    pub user_ids: Vec<RemnaUserId>,
    pub extend_days: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeVersions {
    #[serde(default, deserialize_with = "nullable")]
    pub xray: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub node: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeProvider {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub login_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub uuid: Uuid,
    pub name: String,
    pub address: String,
    #[serde(default, deserialize_with = "nullable")]
    pub port: Option<i64>,
    pub is_connected: bool,
    pub is_disabled: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub is_connecting: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub last_status_change: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "nullable")]
    pub last_status_message: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub traffic_reset_day: Option<i64>,
    #[serde(default, deserialize_with = "nullable")]
    pub traffic_limit_bytes: Option<i64>,
    #[serde(default, deserialize_with = "nullable")]
    pub traffic_used_bytes: Option<i64>,
    #[serde(default, deserialize_with = "nullable")]
    pub notify_percent: Option<i64>,
    #[serde(default, deserialize_with = "nullable")]
    pub country_code: String,
    #[serde(default, deserialize_with = "nullable")]
    pub consumption_multiplier: f64,
    #[serde(default, deserialize_with = "nullable")]
    pub users_online: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub xray_uptime: f64,
    #[serde(default, deserialize_with = "nullable")]
    pub versions: Option<NodeVersions>,
    #[serde(default, deserialize_with = "nullable")]
    pub provider: Option<NodeProvider>,
    #[serde(default, deserialize_with = "nullable")]
    pub tags: Vec<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Online,
    Connecting,
    Down,
    Disabled,
}

impl Node {
    pub const fn state(&self) -> NodeState {
        if self.is_disabled {
            NodeState::Disabled
        } else if self.is_connected {
            NodeState::Online
        } else if self.is_connecting {
            NodeState::Connecting
        } else {
            NodeState::Down
        }
    }

    pub fn traffic_alert(&self) -> bool {
        match (
            self.traffic_used_bytes,
            self.traffic_limit_bytes,
            self.notify_percent,
        ) {
            (Some(used), Some(limit), Some(percent)) if limit > 0 => {
                used as i128 * 100 >= limit as i128 * percent as i128
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inbound {
    pub uuid: Uuid,
    #[serde(default, deserialize_with = "nullable")]
    pub profile_uuid: Option<Uuid>,
    pub tag: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, deserialize_with = "nullable")]
    pub network: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub security: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub port: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadInfo {
    #[serde(default, deserialize_with = "nullable")]
    pub members_count: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub inbounds_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalSquad {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub view_position: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub info: SquadInfo,
    #[serde(default, deserialize_with = "nullable")]
    pub inbounds: Vec<Inbound>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalSquads {
    pub total: i64,
    pub internal_squads: Vec<InternalSquad>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibleNode {
    pub uuid: Uuid,
    pub node_name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub country_code: String,
    #[serde(default, deserialize_with = "nullable")]
    pub config_profile_uuid: Option<Uuid>,
    #[serde(default, deserialize_with = "nullable")]
    pub config_profile_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadAccessibleNodes {
    pub squad_uuid: Uuid,
    pub accessible_nodes: Vec<AccessibleNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserAccessibleNodes {
    pub user_id: RemnaUserId,
    pub active_nodes: Vec<AccessibleNode>,
}

impl UserAccessibleNodes {
    pub fn country_codes(&self) -> Vec<String> {
        let mut codes: Vec<String> = Vec::new();
        for node in &self.active_nodes {
            if !node.country_code.is_empty() && !codes.contains(&node.country_code) {
                codes.push(node.country_code.clone());
            }
        }
        codes
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CustomRemarks {
    #[serde(rename = "expiredUsers", default)]
    pub expired_users: Vec<String>,
    #[serde(rename = "limitedUsers", default)]
    pub limited_users: Vec<String>,
    #[serde(rename = "disabledUsers", default)]
    pub disabled_users: Vec<String>,
    #[serde(rename = "emptyHosts", default)]
    pub empty_hosts: Vec<String>,
    #[serde(rename = "HWIDMaxDevicesExceeded", default)]
    pub hwid_max_devices_exceeded: Vec<String>,
    #[serde(rename = "HWIDNotSupported", default)]
    pub hwid_not_supported: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HwidSettings {
    pub enabled: bool,
    pub fallback_device_limit: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub max_devices_announce: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionSettings {
    pub uuid: Uuid,
    #[serde(default, deserialize_with = "nullable")]
    pub is_show_custom_remarks: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub custom_remarks: CustomRemarks,
    #[serde(default, deserialize_with = "nullable")]
    pub hwid_settings: Option<HwidSettings>,
    #[serde(default, deserialize_with = "nullable")]
    pub randomize_hosts: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub serve_json_at_base_subscription: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSubscriptionSettingsRequest {
    pub uuid: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_show_custom_remarks: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_remarks: Option<CustomRemarks>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hwid_settings: Option<HwidSettings>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMetadata {
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationsConfig {
    #[serde(default, deserialize_with = "nullable")]
    pub webhook: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub expiration_notifications: Vec<i64>,
    #[serde(default, deserialize_with = "nullable")]
    pub not_connected_after: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConfig {
    #[serde(default, deserialize_with = "nullable")]
    pub disable_user_usage_records: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub disable_srh_records: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiscConfig {
    #[serde(default, deserialize_with = "nullable")]
    pub sub_public_domain: String,
    #[serde(default, deserialize_with = "nullable")]
    pub short_uuid_length: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemConfiguration {
    #[serde(default, deserialize_with = "nullable")]
    pub notifications: NotificationsConfig,
    #[serde(default, deserialize_with = "nullable")]
    pub service: ServiceConfig,
    #[serde(default, deserialize_with = "nullable")]
    pub misc: MiscConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsersStats {
    #[serde(default, deserialize_with = "nullable")]
    pub status_counts: HashMap<String, i64>,
    #[serde(default, deserialize_with = "nullable")]
    pub total_users: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineStats {
    #[serde(default, deserialize_with = "nullable")]
    pub online_now: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub last_day: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub last_week: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub never_online: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStats {
    #[serde(default, deserialize_with = "nullable")]
    pub users: UsersStats,
    #[serde(default, deserialize_with = "nullable")]
    pub online_stats: OnlineStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionRequest {
    pub request_at: DateTime<Utc>,
    pub srr_response_type: String,
    #[serde(default, deserialize_with = "nullable")]
    pub request_ip: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub user_agent: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub srr_rule_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionRequests {
    pub total: i64,
    pub records: Vec<SubscriptionRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DateRange {
    pub start: NaiveDate,
    pub end: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageNode {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub country_code: String,
    #[serde(default, deserialize_with = "nullable")]
    pub total: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserUsage {
    #[serde(default, deserialize_with = "nullable")]
    pub categories: Vec<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub sparkline_data: Vec<f64>,
    #[serde(default, deserialize_with = "nullable")]
    pub top_nodes: Vec<UsageNode>,
}

impl UserUsage {
    pub fn total_bytes(&self) -> u64 {
        self.sparkline_data.iter().map(|v| v.max(0.0)).sum::<f64>() as u64
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionsJob {
    pub job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionIp {
    pub ip: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeConnections {
    pub node_uuid: Uuid,
    pub node_name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub country_code: String,
    #[serde(default, deserialize_with = "nullable")]
    pub ips: Vec<ConnectionIp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserConnectionsResult {
    pub success: bool,
    pub user_id: RemnaUserId,
    #[serde(default, deserialize_with = "nullable")]
    pub nodes: Vec<NodeConnections>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct JobProgress {
    #[serde(default, deserialize_with = "nullable")]
    pub total: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub completed: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserConnectionsStatus {
    pub is_completed: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub is_failed: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub progress: JobProgress,
    #[serde(default, deserialize_with = "nullable")]
    pub result: Option<UserConnectionsResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingProvider {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub login_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingNodeRef {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub country_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingNode {
    pub uuid: Uuid,
    #[serde(default, deserialize_with = "nullable")]
    pub node_uuid: Option<Uuid>,
    #[serde(default, deserialize_with = "nullable")]
    pub name: Option<String>,
    pub provider_uuid: Uuid,
    pub provider: BillingProvider,
    #[serde(default, deserialize_with = "nullable")]
    pub node: Option<BillingNodeRef>,
    pub next_billing_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingStats {
    #[serde(default, deserialize_with = "nullable")]
    pub upcoming_nodes_count: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub current_month_payments: f64,
    #[serde(default, deserialize_with = "nullable")]
    pub total_spent: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfraBillingNodes {
    #[serde(default, deserialize_with = "nullable")]
    pub total_billing_nodes: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub billing_nodes: Vec<BillingNode>,
    #[serde(default, deserialize_with = "nullable")]
    pub stats: BillingStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeConfigProfile {
    #[serde(default, deserialize_with = "nullable")]
    pub active_config_profile_uuid: Option<Uuid>,
    #[serde(default, deserialize_with = "nullable")]
    pub active_inbounds: Vec<Inbound>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSystemInfo {
    #[serde(default, deserialize_with = "nullable")]
    pub arch: String,
    #[serde(default, deserialize_with = "nullable")]
    pub cpus: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub cpu_model: String,
    #[serde(default, deserialize_with = "nullable")]
    pub memory_total: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub platform: String,
    #[serde(default, deserialize_with = "nullable")]
    pub release: String,
    #[serde(default, rename = "type")]
    pub os_type: String,
    #[serde(default, deserialize_with = "nullable")]
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSystemStats {
    #[serde(default, deserialize_with = "nullable")]
    pub memory_free: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub memory_used: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub uptime: f64,
    #[serde(default, deserialize_with = "nullable")]
    pub load_avg: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSystem {
    #[serde(default, deserialize_with = "nullable")]
    pub info: NodeSystemInfo,
    #[serde(default, deserialize_with = "nullable")]
    pub stats: NodeSystemStats,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDetails {
    #[serde(flatten)]
    pub node: Node,
    #[serde(default, deserialize_with = "nullable")]
    pub provider_uuid: Option<Uuid>,
    #[serde(default, deserialize_with = "nullable")]
    pub is_traffic_tracking_active: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub view_position: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub config_profile: Option<NodeConfigProfile>,
    #[serde(default, deserialize_with = "nullable")]
    pub system: Option<NodeSystem>,
    #[serde(default, deserialize_with = "nullable")]
    pub created_at: Option<DateTime<Utc>>,
}

impl From<Node> for NodeDetails {
    fn from(node: Node) -> Self {
        NodeDetails {
            provider_uuid: node.provider.as_ref().map(|p| p.uuid),
            node,
            is_traffic_tracking_active: false,
            view_position: 0,
            config_profile: None,
            system: None,
            created_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeProfileSelection {
    pub active_config_profile_uuid: Uuid,
    pub active_inbounds: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNodeRequest {
    pub name: String,
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_traffic_tracking_active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_limit_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_percent: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_reset_day: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumption_multiplier: Option<f64>,
    pub config_profile: NodeProfileSelection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_uuid: Option<Uuid>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl CreateNodeRequest {
    pub fn new(
        name: impl Into<String>,
        address: impl Into<String>,
        profile: NodeProfileSelection,
    ) -> Self {
        CreateNodeRequest {
            name: name.into(),
            address: address.into(),
            port: None,
            country_code: None,
            is_traffic_tracking_active: None,
            traffic_limit_bytes: None,
            notify_percent: None,
            traffic_reset_day: None,
            consumption_multiplier: None,
            config_profile: profile,
            provider_uuid: None,
            tags: Vec::new(),
            note: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNodeRequest {
    pub uuid: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_traffic_tracking_active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_limit_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_percent: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_reset_day: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumption_multiplier: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_profile: Option<NodeProfileSelection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_uuid: Option<Option<Uuid>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<Option<String>>,
}

impl UpdateNodeRequest {
    pub const fn new(uuid: Uuid) -> Self {
        UpdateNodeRequest {
            uuid,
            name: None,
            address: None,
            port: None,
            country_code: None,
            is_traffic_tracking_active: None,
            traffic_limit_bytes: None,
            notify_percent: None,
            traffic_reset_day: None,
            consumption_multiplier: None,
            config_profile: None,
            provider_uuid: None,
            tags: None,
            note: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "KeygenWire")]
pub struct NodeKeygen {
    pub secret_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeygenWire {
    #[serde(default, deserialize_with = "nullable")]
    secret_key: String,
    #[serde(default, deserialize_with = "nullable")]
    pub_key: String,
}

impl TryFrom<KeygenWire> for NodeKeygen {
    type Error = &'static str;

    fn try_from(wire: KeygenWire) -> Result<Self, Self::Error> {
        let secret_key = if wire.secret_key.is_empty() {
            wire.pub_key
        } else {
            wire.secret_key
        };
        if secret_key.is_empty() {
            return Err("в ответе /api/keygen нет ключа ноды");
        }
        Ok(NodeKeygen { secret_key })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigProfileNode {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub country_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigProfile {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub view_position: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub inbounds: Vec<Inbound>,
    #[serde(default, deserialize_with = "nullable")]
    pub nodes: Vec<ConfigProfileNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigProfiles {
    #[serde(default, deserialize_with = "nullable")]
    pub total: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub config_profiles: Vec<ConfigProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInbound {
    #[serde(default, deserialize_with = "nullable")]
    pub config_profile_uuid: Option<Uuid>,
    #[serde(default, deserialize_with = "nullable")]
    pub config_profile_inbound_uuid: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSquadFilter {
    pub mode: String,
    #[serde(default, deserialize_with = "nullable")]
    pub squads: Vec<Uuid>,
}

impl HostSquadFilter {
    pub const EXCLUDE: &'static str = "EXCLUDE";
    pub const ALLOW_ONLY: &'static str = "ALLOW_ONLY";

    pub fn exclude(squads: Vec<Uuid>) -> Self {
        HostSquadFilter {
            mode: Self::EXCLUDE.to_string(),
            squads,
        }
    }

    pub fn allow_only(squads: Vec<Uuid>) -> Self {
        HostSquadFilter {
            mode: Self::ALLOW_ONLY.to_string(),
            squads,
        }
    }

    pub fn is_allow_only(&self) -> bool {
        self.mode == Self::ALLOW_ONLY
    }

    pub fn allows(&self, squad: Uuid) -> bool {
        if self.is_allow_only() {
            self.squads.contains(&squad)
        } else {
            !self.squads.contains(&squad)
        }
    }
}

impl Host {
    pub fn shown_to(&self, squad: Uuid) -> bool {
        !self.is_disabled
            && !self.is_hidden
            && self
                .internal_squads
                .as_ref()
                .is_none_or(|filter| filter.allows(squad))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Host {
    pub uuid: Uuid,
    #[serde(default, deserialize_with = "nullable")]
    pub view_position: i64,
    pub remark: String,
    #[serde(default, deserialize_with = "nullable")]
    pub address: String,
    #[serde(default, deserialize_with = "nullable")]
    pub port: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub is_disabled: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub is_hidden: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub inbound: HostInbound,
    #[serde(default, deserialize_with = "nullable")]
    pub nodes: Vec<Uuid>,
    #[serde(default, deserialize_with = "nullable")]
    pub tags: Vec<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub server_description: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub internal_squads: Option<HostSquadFilter>,
    #[serde(default, deserialize_with = "nullable")]
    pub sni: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub host: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub path: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub alpn: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub fingerprint: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub security_layer: Option<String>,
}

pub const HOST_ALPN: [&str; 6] = [
    "h3",
    "h2",
    "http/1.1",
    "h2,http/1.1",
    "h3,h2,http/1.1",
    "h3,h2",
];

pub const HOST_SECURITY_LAYERS: [&str; 3] = ["DEFAULT", "TLS", "NONE"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateHostRequest {
    pub uuid: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remark: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpn: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_layer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_disabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_squads: Option<HostSquadFilter>,
}

impl UpdateHostRequest {
    pub const fn new(uuid: Uuid) -> Self {
        UpdateHostRequest {
            uuid,
            remark: None,
            address: None,
            port: None,
            sni: None,
            host: None,
            path: None,
            alpn: None,
            fingerprint: None,
            security_layer: None,
            is_disabled: None,
            is_hidden: None,
            internal_squads: None,
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.remark.is_none()
            && self.address.is_none()
            && self.port.is_none()
            && self.sni.is_none()
            && self.host.is_none()
            && self.path.is_none()
            && self.alpn.is_none()
            && self.fingerprint.is_none()
            && self.security_layer.is_none()
            && self.is_disabled.is_none()
            && self.is_hidden.is_none()
            && self.internal_squads.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInboundTarget {
    pub config_profile_uuid: Uuid,
    pub config_profile_inbound_uuid: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateHostRequest {
    pub inbound: HostInboundTarget,
    pub remark: String,
    pub address: String,
    pub port: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_layer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_disabled: Option<bool>,
}

impl CreateHostRequest {
    pub fn new(
        inbound: HostInboundTarget,
        remark: impl Into<String>,
        address: impl Into<String>,
        port: i64,
    ) -> Self {
        CreateHostRequest {
            inbound,
            remark: remark.into(),
            address: address.into(),
            port,
            sni: None,
            host: None,
            path: None,
            alpn: None,
            fingerprint: None,
            security_layer: None,
            is_disabled: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPosition {
    pub uuid: Uuid,
    pub view_position: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostsReordered {
    #[serde(default, deserialize_with = "nullable")]
    pub is_updated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInternalSquadRequest {
    pub name: String,
    pub inbounds: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInternalSquadRequest {
    pub uuid: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inbounds: Option<Vec<Uuid>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadUsageQuery {
    pub start: NaiveDate,
    pub end: NaiveDate,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_total_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadUserUsage {
    pub id: RemnaUserId,
    #[serde(default, deserialize_with = "nullable")]
    pub total_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadUsage {
    pub squad_uuid: Uuid,
    #[serde(default, deserialize_with = "nullable")]
    pub users: Vec<SquadUserUsage>,
    #[serde(default, deserialize_with = "nullable")]
    pub next_cursor: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub has_more: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalSquadInfo {
    #[serde(default, deserialize_with = "nullable")]
    pub members_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalSquad {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub view_position: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub info: ExternalSquadInfo,
    #[serde(default, deserialize_with = "nullable")]
    pub hwid_settings: Option<HwidSettings>,
    #[serde(default, deserialize_with = "nullable")]
    pub custom_remarks: Option<CustomRemarks>,
    #[serde(default, deserialize_with = "nullable")]
    pub subpage_config_uuid: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalSquads {
    #[serde(default, deserialize_with = "nullable")]
    pub total: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub external_squads: Vec<ExternalSquad>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct BillingAmount(pub i64);

impl BillingAmount {
    pub const fn from_minor(minor: i64) -> Self {
        BillingAmount(minor)
    }

    pub const fn minor(self) -> i64 {
        self.0
    }

    pub const fn money(self, currency: Currency) -> Money {
        Money::new(self.0, currency)
    }

    pub fn units(self) -> String {
        minor_to_units(self.0, PayCurrency::Usd)
    }

    pub fn parse(raw: &str) -> Option<Self> {
        decimal_to_minor(raw).map(BillingAmount)
    }
}

impl From<Money> for BillingAmount {
    fn from(value: Money) -> Self {
        BillingAmount(value.minor)
    }
}

fn decimal_to_minor(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    let (negative, body) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, raw),
    };
    let (mantissa, exponent) = match body.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (body, 0),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    let digits: Vec<u8> = int_part.bytes().chain(frac_part.bytes()).collect();
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let point = i32::try_from(int_part.len())
        .ok()?
        .checked_add(exponent)?
        .checked_add(2)?;
    let mut value: i128 = 0;
    let kept = usize::try_from(point.max(0)).ok()?;
    for index in 0..kept {
        let digit = digits.get(index).map_or(0, |d| i128::from(d - b'0'));
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    if point >= 0 && digits.get(kept).is_some_and(|d| *d >= b'5') {
        value = value.checked_add(1)?;
    }
    let value = i64::try_from(value).ok()?;
    Some(if negative { -value } else { value })
}

impl Serialize for BillingAmount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let number: serde_json::Number = self.units().parse().map_err(serde::ser::Error::custom)?;
        number.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BillingAmount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let number = serde_json::Number::deserialize(deserializer)?;
        BillingAmount::parse(&number.to_string())
            .ok_or_else(|| serde::de::Error::custom(format!("сумма {number} не разобрана")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBillingSummary {
    #[serde(default, deserialize_with = "nullable")]
    pub total_amount: BillingAmount,
    #[serde(default, deserialize_with = "nullable")]
    pub total_bills: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBillingNodeDetails {
    pub node_uuid: Uuid,
    #[serde(default, deserialize_with = "nullable")]
    pub country_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBillingNode {
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub details: Option<ProviderBillingNodeDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfraProvider {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub favicon_link: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub login_url: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub billing_history: ProviderBillingSummary,
    #[serde(default, deserialize_with = "nullable")]
    pub billing_nodes: Vec<ProviderBillingNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfraProviders {
    #[serde(default, deserialize_with = "nullable")]
    pub total: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub providers: Vec<InfraProvider>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInfraProviderRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favicon_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfraProviderRequest {
    pub uuid: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favicon_link: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_url: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingRecordProvider {
    pub uuid: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingRecord {
    pub uuid: Uuid,
    pub provider_uuid: Uuid,
    pub amount: BillingAmount,
    pub billed_at: DateTime<Utc>,
    #[serde(default, deserialize_with = "nullable")]
    pub provider: Option<BillingRecordProvider>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfraBillingHistory {
    #[serde(default, deserialize_with = "nullable")]
    pub records: Vec<BillingRecord>,
    #[serde(default, deserialize_with = "nullable")]
    pub total: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingHistoryQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBillingRecordRequest {
    pub provider_uuid: Uuid,
    pub amount: BillingAmount,
    pub billed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBillingNodeRequest {
    pub provider_uuid: Uuid,
    pub node_uuid: Option<Uuid>,
    pub name: Option<String>,
    pub next_billing_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBillingNodesRequest {
    pub uuids: Vec<Uuid>,
    pub next_billing_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NodeMetadata {
    #[serde(default, deserialize_with = "nullable")]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl NodeMetadata {
    pub const CITY: &'static str = "city";

    pub fn city(&self) -> Option<&str> {
        self.metadata
            .get(Self::CITY)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|city| !city.is_empty())
    }

    pub fn set_city(&mut self, city: Option<&str>) {
        match city.map(str::trim).filter(|c| !c.is_empty()) {
            Some(city) => {
                self.metadata.insert(
                    Self::CITY.to_string(),
                    serde_json::Value::String(city.to_string()),
                );
            }
            None => {
                self.metadata.remove(Self::CITY);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubpageConfigRef {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub view_position: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubpageConfig {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub view_position: i64,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubpageConfigs {
    #[serde(default, deserialize_with = "nullable")]
    pub total: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub configs: Vec<SubpageConfigRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubpageAccess {
    #[serde(default, deserialize_with = "nullable")]
    pub subpage_config_uuid: Option<Uuid>,
    #[serde(default, deserialize_with = "nullable")]
    pub webpage_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeUserIps {
    pub user_id: RemnaUserId,
    #[serde(default, deserialize_with = "nullable")]
    pub ips: Vec<ConnectionIp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeConnectionsResult {
    pub success: bool,
    pub node_uuid: Uuid,
    #[serde(default, deserialize_with = "nullable")]
    pub users: Vec<NodeUserIps>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeConnectionsStatus {
    pub is_completed: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub is_failed: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub result: Option<NodeConnectionsResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodesUsageQuery {
    pub start: NaiveDate,
    pub end: NaiveDate,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_nodes_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeUsageSeries {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub country_code: String,
    #[serde(default, deserialize_with = "nullable")]
    pub total: f64,
    #[serde(default, deserialize_with = "nullable")]
    pub data: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodesUsage {
    #[serde(default, deserialize_with = "nullable")]
    pub categories: Vec<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub sparkline_data: Vec<f64>,
    #[serde(default, deserialize_with = "nullable")]
    pub top_nodes: Vec<UsageNode>,
    #[serde(default, deserialize_with = "nullable")]
    pub series: Vec<NodeUsageSeries>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BandwidthPeriod {
    #[serde(default, deserialize_with = "nullable")]
    pub current: String,
    #[serde(default, deserialize_with = "nullable")]
    pub previous: String,
    #[serde(default, deserialize_with = "nullable")]
    pub difference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BandwidthStats {
    #[serde(rename = "bandwidthLastTwoDays", default)]
    pub last_two_days: BandwidthPeriod,
    #[serde(rename = "bandwidthLastSevenDays", default)]
    pub last_seven_days: BandwidthPeriod,
    #[serde(rename = "bandwidthLast30Days", default)]
    pub last_30_days: BandwidthPeriod,
    #[serde(rename = "bandwidthCalendarMonth", default)]
    pub calendar_month: BandwidthPeriod,
    #[serde(rename = "bandwidthCurrentYear", default)]
    pub current_year: BandwidthPeriod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficCounter {
    pub tag: String,
    #[serde(default, deserialize_with = "nullable")]
    pub upload: String,
    #[serde(default, deserialize_with = "nullable")]
    pub download: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeMetrics {
    pub node_uuid: String,
    pub node_name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub provider_name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub users_online: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub inbounds_stats: Vec<TrafficCounter>,
    #[serde(default, deserialize_with = "nullable")]
    pub outbounds_stats: Vec<TrafficCounter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodesMetrics {
    #[serde(default, deserialize_with = "nullable")]
    pub nodes: Vec<NodeMetrics>,
}

pub(crate) fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn lenient_datetime<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<DateTime<Utc>>, D::Error> {
    let raw: Option<String> = Option::deserialize(deserializer)?;
    Ok(raw.and_then(|value| {
        DateTime::parse_from_rfc3339(value.trim())
            .ok()
            .map(|at| at.with_timezone(&Utc))
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentReportsQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentUserRef {
    #[serde(default, deserialize_with = "nullable")]
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentNodeRef {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub country_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentActionReport {
    #[serde(default, deserialize_with = "nullable")]
    pub blocked: bool,
    pub ip: String,
    #[serde(default, deserialize_with = "nullable")]
    pub block_duration: i64,
    #[serde(default, deserialize_with = "lenient_datetime")]
    pub will_unblock_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "lenient_datetime")]
    pub processed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentReportDetails {
    pub action_report: TorrentActionReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentReport {
    pub id: i64,
    pub user_id: RemnaUserId,
    #[serde(default, deserialize_with = "nullable")]
    pub user: TorrentUserRef,
    pub node: TorrentNodeRef,
    pub report: TorrentReportDetails,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentReports {
    #[serde(default, deserialize_with = "nullable")]
    pub records: Vec<TorrentReport>,
    #[serde(default, deserialize_with = "nullable")]
    pub total: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentStatsSummary {
    #[serde(default, deserialize_with = "nullable")]
    pub distinct_nodes: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub distinct_users: i64,
    #[serde(default, deserialize_with = "nullable")]
    pub total_reports: i64,
    #[serde(rename = "reportsLast24Hours", default)]
    pub reports_last_24_hours: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentTopUser {
    pub user_id: RemnaUserId,
    #[serde(default, deserialize_with = "nullable")]
    pub username: String,
    #[serde(default, deserialize_with = "nullable")]
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentTopNode {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub country_code: String,
    #[serde(default, deserialize_with = "nullable")]
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentStats {
    #[serde(default, deserialize_with = "nullable")]
    pub stats: TorrentStatsSummary,
    #[serde(default, deserialize_with = "nullable")]
    pub top_users: Vec<TorrentTopUser>,
    #[serde(default, deserialize_with = "nullable")]
    pub top_nodes: Vec<TorrentTopNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "command")]
pub enum PluginCommand {
    #[serde(rename = "unblockIps")]
    UnblockIps { ips: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginExecutorRequest {
    pub command: PluginCommand,
    pub target_nodes: TargetNodes,
}

impl PluginExecutorRequest {
    pub fn unblock_everywhere(ips: Vec<String>) -> Self {
        PluginExecutorRequest {
            command: PluginCommand::UnblockIps { ips },
            target_nodes: TargetNodes::AllNodes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ключ_ноды_берётся_из_secret_key_даже_когда_рядом_pub_key() {
        let both: NodeKeygen = serde_json::from_value(serde_json::json!({
            "secretKey": "eyJub2RlQ2VydFBlbSI6",
            "pubKey": "MCowBQYDK2VwAyEA"
        }))
        .unwrap();
        assert_eq!(both.secret_key, "eyJub2RlQ2VydFBlbSI6");
        let old: NodeKeygen =
            serde_json::from_value(serde_json::json!({"pubKey": "MCowBQYDK2VwAyEA"})).unwrap();
        assert_eq!(old.secret_key, "MCowBQYDK2VwAyEA");
        let empty_pub: NodeKeygen = serde_json::from_value(
            serde_json::json!({"secretKey": "eyJub2RlQ2VydFBlbSI6", "pubKey": null}),
        )
        .unwrap();
        assert_eq!(empty_pub.secret_key, "eyJub2RlQ2VydFBlbSI6");
        assert!(serde_json::from_value::<NodeKeygen>(serde_json::json!({})).is_err());
        assert!(
            serde_json::from_value::<NodeKeygen>(
                serde_json::json!({"secretKey": "", "pubKey": ""})
            )
            .is_err()
        );
        assert_eq!(
            serde_json::to_value(&both).unwrap(),
            serde_json::json!({"secretKey": "eyJub2RlQ2VydFBlbSI6"})
        );
    }

    #[test]
    fn сброс_соединений_сериализуется_как_в_спецификации() {
        let body =
            serde_json::to_value(DropConnectionsRequest::users_everywhere(vec![RemnaUserId(
                7,
            )]))
            .unwrap();
        assert_eq!(
            body,
            serde_json::json!({"dropBy": {"by": "userIds", "userIds": [7]}, "targetNodes": {"target": "allNodes"}})
        );
    }

    #[test]
    fn null_вместо_списков_и_строк_не_ломает_разбор() {
        let config: SystemConfiguration = serde_json::from_value(serde_json::json!({
            "notifications": {
                "webhook": true,
                "expirationNotifications": null,
                "notConnectedAfter": null
            },
            "service": { "disableUserUsageRecords": null, "disableSrhRecords": false },
            "misc": { "subPublicDomain": null, "shortUuidLength": 16 }
        }))
        .unwrap();
        assert!(config.notifications.webhook);
        assert!(config.notifications.expiration_notifications.is_empty());
        assert!(config.notifications.not_connected_after.is_empty());
        assert!(!config.service.disable_user_usage_records);
        assert_eq!(config.misc.sub_public_domain, "");
        assert_eq!(config.misc.short_uuid_length, 16);
        let empty: SystemConfiguration = serde_json::from_value(serde_json::json!({
            "notifications": null,
            "service": null,
            "misc": null
        }))
        .unwrap();
        assert_eq!(empty, SystemConfiguration::default());
    }

    #[test]
    fn хост_с_null_вместо_строк_и_списков_разбирается() {
        let host: Host = serde_json::from_value(serde_json::json!({
            "uuid": "00000000-0000-4000-8000-000000000071", "viewPosition": null, "remark": "Германия",
            "address": null, "port": null, "path": null, "sni": null, "host": null, "alpn": null,
            "fingerprint": null, "isDisabled": null, "securityLayer": null, "xhttpExtraParams": null,
            "muxParams": null, "sockoptParams": null, "finalMask": null,
            "inbound": {"configProfileUuid": null, "configProfileInboundUuid": null},
            "serverDescription": null, "tags": null, "isHidden": null, "nodes": null,
            "xrayJsonTemplateUuid": null, "excludeFromSubscriptionTypes": null, "mapper": null,
            "includeInSrrRules": null, "internalSquads": null
        }))
        .unwrap();
        assert_eq!(host.remark, "Германия");
        assert_eq!(host.address, "");
        assert_eq!(host.port, 0);
        assert!(host.nodes.is_empty() && host.tags.is_empty());
        assert_eq!(host.inbound, HostInbound::default());
        assert_eq!(
            (host.sni, host.alpn, host.security_layer),
            (None, None, None)
        );
        let empty: Host = serde_json::from_value(serde_json::json!({
            "uuid": "00000000-0000-4000-8000-000000000072", "remark": "Нидерланды", "inbound": null
        }))
        .unwrap();
        assert!(!empty.is_disabled && !empty.is_hidden);
        assert!(empty.internal_squads.is_none());
        let full: Host = serde_json::from_value(serde_json::json!({
            "uuid": "00000000-0000-4000-8000-000000000073", "viewPosition": 4, "remark": "Финляндия",
            "address": "fi.example.com", "port": 8443, "sni": "www.example.com", "host": "cdn.example.com",
            "path": "/xhttp", "alpn": "h3,h2", "fingerprint": "chrome", "securityLayer": "TLS",
            "isDisabled": true, "inbound": {"configProfileUuid": "00000000-0000-4000-8000-000000000021",
            "configProfileInboundUuid": "00000000-0000-4000-8000-000000000031"}
        }))
        .unwrap();
        assert_eq!(full.port, 8443);
        assert_eq!(full.sni.as_deref(), Some("www.example.com"));
        assert_eq!(full.alpn.as_deref(), Some("h3,h2"));
        assert_eq!(full.security_layer.as_deref(), Some("TLS"));
        let back = serde_json::to_value(&full).unwrap();
        assert_eq!(back["securityLayer"], "TLS");
        assert_eq!(back["fingerprint"], "chrome");
    }

    #[test]
    fn правка_хоста_отличает_null_от_отсутствия_поля() {
        let mut req = UpdateHostRequest::new(Uuid::from_u128(7));
        assert!(req.is_empty());
        req.sni = Some(None);
        req.port = Some(8443);
        req.alpn = Some(Some("h2".into()));
        assert!(!req.is_empty());
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            serde_json::json!({"uuid": "00000000-0000-0000-0000-000000000007", "port": 8443, "sni": null, "alpn": "h2"})
        );
        let target = HostInboundTarget {
            config_profile_uuid: Uuid::from_u128(1),
            config_profile_inbound_uuid: Uuid::from_u128(2),
        };
        let mut create = CreateHostRequest::new(target, "Германия", "de.example.com", 443);
        create.fingerprint = Some("chrome".into());
        assert_eq!(
            serde_json::to_value(&create).unwrap(),
            serde_json::json!({
                "inbound": {"configProfileUuid": "00000000-0000-0000-0000-000000000001",
                            "configProfileInboundUuid": "00000000-0000-0000-0000-000000000002"},
                "remark": "Германия", "address": "de.example.com", "port": 443, "fingerprint": "chrome"
            })
        );
    }

    #[test]
    fn ключ_ноды_читается_из_обоих_имён_поля() {
        for body in [
            serde_json::json!({"secretKey": "a"}),
            serde_json::json!({"pubKey": "a"}),
        ] {
            let key: NodeKeygen = serde_json::from_value(body).unwrap();
            assert_eq!(key.secret_key, "a");
        }
    }

    #[test]
    fn патч_отличает_null_от_отсутствия_поля() {
        let mut req = UpdateUserRequest::new(RemnaUserId(5));
        req.tag = Some(None);
        req.hwid_device_limit = Some(Some(5));
        let body = serde_json::to_value(&req).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"id": 5, "tag": null, "hwidDeviceLimit": 5})
        );
    }

    #[test]
    fn пользователь_без_секретов_и_с_пустыми_сквадами() {
        let raw = serde_json::json!({
            "id": 42, "shortUuid": "abc", "username": "sn_x", "status": "LIMITED",
            "trafficLimitBytes": 322122547200_i64, "trafficLimitStrategy": "MONTH",
            "expireAt": "2027-02-14T00:00:00.000Z", "createdAt": "2026-11-16T14:52:00.000Z",
            "updatedAt": "2026-11-16T14:52:00.000Z", "vlessUuid": "00000000-0000-0000-0000-000000000000",
            "trojanPassword": "secret", "ssPassword": "secret", "subscriptionUrl": "https://sub.example.com/abc",
            "userTraffic": {"usedTrafficBytes": 1, "lifetimeUsedTrafficBytes": 2, "onlineAt": null, "firstConnectedAt": null, "lastConnectedNodeUuid": null}
        });
        let user: RemnaUser = serde_json::from_value(raw).unwrap();
        assert_eq!(user.state().status, SubscriptionStatus::Limited);
        assert!(user.active_internal_squads.is_empty());
        assert_eq!(
            user.keys().trojan_password.as_ref().map(Secret::expose),
            Some("secret")
        );
        assert!(!format!("{user:?}").contains("secret"));
        let back = serde_json::to_string(&user).unwrap();
        assert!(!back.contains("secret"));
        assert!(!back.contains("vlessUuid"));
    }

    #[test]
    fn сумма_оплаты_сервера_без_плавающей_точки() {
        let parse = |raw: &str| BillingAmount::parse(raw).map(BillingAmount::minor);
        assert_eq!(parse("7.59"), Some(759));
        assert_eq!(parse("1300"), Some(130_000));
        assert_eq!(parse("1300.0"), Some(130_000));
        assert_eq!(parse("7.595"), Some(760));
        assert_eq!(parse("7.594"), Some(759));
        assert_eq!(parse("-7.59"), Some(-759));
        assert_eq!(parse("1e3"), Some(100_000));
        assert_eq!(parse("5e-3"), Some(1));
        assert_eq!(parse(".5"), Some(50));
        assert_eq!(parse("abc"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("1.2.3"), None);
        let amount: BillingAmount = serde_json::from_str("80.09").unwrap();
        assert_eq!(amount, BillingAmount(8009));
        assert_eq!(serde_json::to_string(&amount).unwrap(), "80.09");
        assert_eq!(
            serde_json::to_string(&BillingAmount(130_000)).unwrap(),
            "1300.0"
        );
        assert_eq!(amount.money(Currency::Usd), Money::new(8009, Currency::Usd));
        assert!(serde_json::from_str::<BillingAmount>("\"7.59\"").is_err());
    }

    #[test]
    fn подробности_ноды_дополняют_общую_модель() {
        let raw = serde_json::json!({
            "uuid": "00000000-0000-0000-0000-000000000001", "name": "DE-1", "address": "203.0.113.41",
            "isConnected": true, "isDisabled": false, "countryCode": "DE",
            "providerUuid": "00000000-0000-0000-0000-000000000002", "isTrafficTrackingActive": true,
            "configProfile": {"activeConfigProfileUuid": null, "activeInbounds": []},
            "system": null
        });
        let details: NodeDetails = serde_json::from_value(raw).unwrap();
        assert_eq!(details.node.state(), NodeState::Online);
        assert!(details.is_traffic_tracking_active);
        assert!(details.system.is_none());
        let back = serde_json::to_value(&details).unwrap();
        assert_eq!(back["name"], "DE-1");
        assert_eq!(back["providerUuid"], "00000000-0000-0000-0000-000000000002");
        let from_node = NodeDetails::from(details.node.clone());
        assert_eq!(from_node.node, details.node);
    }

    #[test]
    fn состояние_ноды() {
        let mut node: Node = serde_json::from_value(serde_json::json!({
            "uuid": "00000000-0000-0000-0000-000000000001", "name": "NL-2", "address": "198.51.100.22",
            "isConnected": false, "isDisabled": false, "isConnecting": false,
            "trafficLimitBytes": 100, "trafficUsedBytes": 92, "notifyPercent": 90
        }))
        .unwrap();
        assert_eq!(node.state(), NodeState::Down);
        assert!(node.traffic_alert());
        node.is_disabled = true;
        assert_eq!(node.state(), NodeState::Disabled);
    }
}
