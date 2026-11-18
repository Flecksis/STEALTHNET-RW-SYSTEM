use crate::api::RemnawaveApi;
use crate::error::{RemnaError, RemnaResult};
use crate::models::*;
use async_trait::async_trait;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use sn_domain::ids::RemnaUserId;
use sn_domain::traffic::TrafficStrategy;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Timeout,
    TimeoutAfterApply,
    Unavailable,
    ServerError,
    Unauthorized,
    RateLimited,
}

impl Fault {
    pub fn error(self) -> RemnaError {
        match self {
            Fault::Timeout | Fault::TimeoutAfterApply => RemnaError::Timeout,
            Fault::Unavailable => RemnaError::Unavailable("fake".into()),
            Fault::ServerError => RemnaError::Server {
                status: 500,
                message: "fake".into(),
            },
            Fault::Unauthorized => RemnaError::Unauthorized(401),
            Fault::RateLimited => RemnaError::RateLimited,
        }
    }

    pub const fn applies(self) -> bool {
        matches!(self, Fault::TimeoutAfterApply)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FakeCall {
    pub method: String,
    pub args: Value,
}

pub type HookFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type CreateHook = Arc<dyn Fn(RemnaUser) -> HookFuture + Send + Sync>;

#[derive(Default)]
struct State {
    next_id: i64,
    users: BTreeMap<i64, RemnaUser>,
    devices: BTreeMap<i64, Vec<HwidDevice>>,
    nodes: Vec<NodeDetails>,
    squads: Vec<InternalSquad>,
    external_squads: Vec<ExternalSquad>,
    hosts: Vec<Host>,
    config_profiles: Vec<ConfigProfile>,
    providers: Vec<InfraProvider>,
    billing_nodes: Vec<BillingNode>,
    history: Vec<BillingRecord>,
    metadata: BTreeMap<Uuid, NodeMetadata>,
    subpage_configs: Vec<SubpageConfigRef>,
    subpage_contents: BTreeMap<Uuid, Value>,
    settings: Option<SubscriptionSettings>,
    torrent_reports: Vec<TorrentReport>,
    unblocked_ips: Vec<String>,
    faults: VecDeque<Fault>,
    targeted: Vec<(String, Fault)>,
    calls: Vec<FakeCall>,
    create_hook: Option<CreateHook>,
}

impl State {
    fn take_fault(&mut self, call: &str) -> Option<Fault> {
        if let Some(index) = self.targeted.iter().position(|(method, _)| method == call) {
            return Some(self.targeted.remove(index).1);
        }
        self.faults.pop_front()
    }

    fn node_mut(&mut self, uuid: Uuid) -> RemnaResult<&mut NodeDetails> {
        self.nodes
            .iter_mut()
            .find(|n| n.node.uuid == uuid)
            .ok_or_else(|| not_found("Node"))
    }

    fn inbounds(&self, uuids: &[Uuid]) -> Vec<Inbound> {
        self.config_profiles
            .iter()
            .flat_map(|p| p.inbounds.iter())
            .filter(|i| uuids.contains(&i.uuid))
            .cloned()
            .collect()
    }

    fn history(&self) -> InfraBillingHistory {
        InfraBillingHistory {
            records: self.history.clone(),
            total: self.history.len() as i64,
        }
    }

    fn billing(&self) -> InfraBillingNodes {
        InfraBillingNodes {
            total_billing_nodes: self.billing_nodes.len() as i64,
            billing_nodes: self.billing_nodes.clone(),
            stats: BillingStats::default(),
        }
    }
}

#[derive(Default)]
pub struct FakeRemnawave {
    state: Mutex<State>,
}

pub fn panel_time(at: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(at.timestamp_millis()).unwrap_or(at)
}

pub fn sample_user(id: i64, username: &str) -> RemnaUser {
    let now = Utc::now();
    RemnaUser {
        id: RemnaUserId(id),
        short_uuid: format!("short{id}"),
        username: username.to_string(),
        status: UserStatus::Active,
        traffic_limit_bytes: 0,
        traffic_limit_strategy: TrafficStrategy::NoReset,
        expire_at: panel_time(now + Duration::days(30)),
        telegram_id: None,
        email: None,
        description: None,
        tag: None,
        hwid_device_limit: None,
        external_squad_uuid: None,
        sub_revoked_at: None,
        last_traffic_reset_at: None,
        created_at: now,
        updated_at: now,
        subscription_url: format!("https://sub.example.com/short{id}"),
        active_internal_squads: Vec::new(),
        user_traffic: UserTraffic::default(),
    }
}

pub fn sample_node(uuid: Uuid, name: &str, country_code: &str) -> Node {
    Node {
        uuid,
        name: name.to_string(),
        address: "198.51.100.10".into(),
        port: Some(2222),
        is_connected: true,
        is_disabled: false,
        is_connecting: false,
        last_status_change: None,
        last_status_message: None,
        traffic_reset_day: None,
        traffic_limit_bytes: None,
        traffic_used_bytes: None,
        notify_percent: None,
        country_code: country_code.to_string(),
        consumption_multiplier: 1.0,
        users_online: 0,
        xray_uptime: 0.0,
        versions: None,
        provider: None,
        tags: Vec::new(),
        note: None,
    }
}

pub fn sample_device(user_id: RemnaUserId, hwid: &str, created_at: DateTime<Utc>) -> HwidDevice {
    HwidDevice {
        hwid: hwid.to_string(),
        user_id,
        platform: Some("iOS".into()),
        os_version: Some("18.1".into()),
        device_model: Some("iPhone 15 Pro".into()),
        user_agent: Some("Happ/3.2.1".into()),
        request_ip: None,
        created_at,
        updated_at: created_at,
    }
}

pub fn webhook_body(scope: &str, event: &str, at: DateTime<Utc>, data: Value) -> Vec<u8> {
    json!({
        "scope": scope,
        "event": event,
        "timestamp": at.to_rfc3339_opts(SecondsFormat::Millis, true),
        "data": data,
    })
    .to_string()
    .into_bytes()
}

pub fn user_webhook(event: &str, user: &RemnaUser, at: DateTime<Utc>) -> Vec<u8> {
    let mut data = serde_json::to_value(user).unwrap_or(Value::Null);
    if let Some(object) = data.as_object_mut() {
        object.insert("vlessUuid".into(), json!(Uuid::nil()));
        object.insert("trojanPassword".into(), json!("fake-trojan-password"));
        object.insert("ssPassword".into(), json!("fake-ss-password"));
    }
    let scope = event.split_once('.').map_or("user", |(scope, _)| scope);
    webhook_body(scope, event, at, data)
}

fn not_found(what: &str) -> RemnaError {
    RemnaError::NotFound {
        code: None,
        message: format!("{what} not found"),
    }
}

fn squad_refs(uuids: &[Uuid]) -> Vec<SquadRef> {
    uuids
        .iter()
        .map(|uuid| SquadRef {
            uuid: *uuid,
            name: uuid.to_string(),
        })
        .collect()
}

fn args<T: Serialize + ?Sized>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn new_short_uuid() -> String {
    Uuid::new_v4().simple().to_string()[..16].to_string()
}

impl FakeRemnawave {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn run<T>(
        &self,
        call: &str,
        call_args: Value,
        op: impl FnOnce(&mut State) -> RemnaResult<T>,
    ) -> RemnaResult<T> {
        let mut state = self.lock();
        state.calls.push(FakeCall {
            method: call.to_string(),
            args: call_args,
        });
        match state.take_fault(call) {
            Some(fault) if fault.applies() => op(&mut state).and(Err(fault.error())),
            Some(fault) => Err(fault.error()),
            None => op(&mut state),
        }
    }

    fn with_user<T>(
        &self,
        call: &str,
        call_args: Value,
        id: RemnaUserId,
        op: impl FnOnce(&mut RemnaUser) -> RemnaResult<T>,
    ) -> RemnaResult<T> {
        self.run(call, call_args, |state| {
            let user = state
                .users
                .get_mut(&id.0)
                .ok_or_else(|| not_found("User"))?;
            let result = op(user)?;
            user.updated_at = Utc::now();
            Ok(result)
        })
    }

    pub fn insert_user(&self, mut user: RemnaUser) {
        user.expire_at = panel_time(user.expire_at);
        let mut state = self.lock();
        state.next_id = state.next_id.max(user.id.0);
        state.users.insert(user.id.0, user);
    }

    pub fn user(&self, id: RemnaUserId) -> Option<RemnaUser> {
        self.lock().users.get(&id.0).cloned()
    }

    pub fn users(&self) -> Vec<RemnaUser> {
        self.lock().users.values().cloned().collect()
    }

    pub fn add_device(&self, device: HwidDevice) {
        self.lock()
            .devices
            .entry(device.user_id.0)
            .or_default()
            .push(device);
    }

    pub fn devices(&self, id: RemnaUserId) -> Vec<HwidDevice> {
        self.lock().devices.get(&id.0).cloned().unwrap_or_default()
    }

    pub fn set_nodes(&self, nodes: Vec<Node>) {
        self.lock().nodes = nodes.into_iter().map(NodeDetails::from).collect();
    }

    pub fn set_node_details(&self, nodes: Vec<NodeDetails>) {
        self.lock().nodes = nodes;
    }

    pub fn node_details(&self) -> Vec<NodeDetails> {
        self.lock().nodes.clone()
    }

    pub fn set_squads(&self, squads: Vec<InternalSquad>) {
        self.lock().squads = squads;
    }

    pub fn squads(&self) -> Vec<InternalSquad> {
        self.lock().squads.clone()
    }

    pub fn set_external_squads(&self, squads: Vec<ExternalSquad>) {
        self.lock().external_squads = squads;
    }

    pub fn set_hosts(&self, hosts: Vec<Host>) {
        self.lock().hosts = hosts;
    }

    pub fn host_list(&self) -> Vec<Host> {
        self.lock().hosts.clone()
    }

    pub fn set_config_profiles(&self, profiles: Vec<ConfigProfile>) {
        self.lock().config_profiles = profiles;
    }

    pub fn set_subscription_settings(&self, settings: SubscriptionSettings) {
        self.lock().settings = Some(settings);
    }

    pub fn set_subpage_configs(&self, configs: Vec<SubpageConfigRef>) {
        self.lock().subpage_configs = configs;
    }

    pub fn set_subpage_content(&self, uuid: Uuid, config: Value) {
        self.lock().subpage_contents.insert(uuid, config);
    }

    pub fn add_torrent_report(&self, report: TorrentReport) {
        self.lock().torrent_reports.push(report);
    }

    pub fn unblocked_ips(&self) -> Vec<String> {
        self.lock().unblocked_ips.clone()
    }

    pub fn billing_records(&self) -> Vec<BillingRecord> {
        self.lock().history.clone()
    }

    pub fn fail_next(&self, fault: Fault) {
        self.lock().faults.push_back(fault);
    }

    pub fn fail_on(&self, method: &str, fault: Fault) {
        self.lock().targeted.push((method.to_string(), fault));
    }

    pub fn calls(&self) -> Vec<String> {
        self.lock().calls.iter().map(|c| c.method.clone()).collect()
    }

    pub fn call_log(&self) -> Vec<FakeCall> {
        self.lock().calls.clone()
    }

    pub fn calls_of(&self, method: &str) -> Vec<Value> {
        self.lock()
            .calls
            .iter()
            .filter(|c| c.method == method)
            .map(|c| c.args.clone())
            .collect()
    }

    pub fn before_create_response<F>(&self, hook: F)
    where
        F: Fn(RemnaUser) -> HookFuture + Send + Sync + 'static,
    {
        self.lock().create_hook = Some(Arc::new(hook));
    }

    fn devices_of(state: &State, id: RemnaUserId) -> HwidDevices {
        let devices = state.devices.get(&id.0).cloned().unwrap_or_default();
        HwidDevices {
            total: devices.len() as i64,
            devices,
        }
    }

    fn create_user_now(state: &mut State, request: &CreateUserRequest) -> RemnaResult<RemnaUser> {
        if state.users.values().any(|u| u.username == request.username) {
            return Err(RemnaError::BadRequest {
                code: Some("A019".into()),
                message: "User username already exists".into(),
            });
        }
        state.next_id += 1;
        let mut user = sample_user(state.next_id, &request.username);
        user.status = request.status.unwrap_or(UserStatus::Active);
        user.expire_at = panel_time(request.expire_at);
        user.traffic_limit_bytes = request.traffic_limit_bytes;
        user.traffic_limit_strategy = request.traffic_limit_strategy;
        user.description = request.description.clone();
        user.tag = request.tag.clone();
        user.telegram_id = request.telegram_id;
        user.email = request.email.clone();
        user.hwid_device_limit = request.hwid_device_limit;
        user.external_squad_uuid = request.external_squad_uuid;
        user.active_internal_squads = squad_refs(&request.active_internal_squads);
        state.users.insert(user.id.0, user.clone());
        Ok(user)
    }
}

#[async_trait]
impl RemnawaveApi for FakeRemnawave {
    async fn metadata(&self) -> RemnaResult<SystemMetadata> {
        self.run("metadata", Value::Null, |_| {
            Ok(SystemMetadata {
                version: crate::SUPPORTED_VERSION.into(),
            })
        })
    }

    async fn auth_status(&self) -> RemnaResult<()> {
        self.run("auth_status", Value::Null, |_| Ok(()))
    }

    async fn configuration(&self) -> RemnaResult<SystemConfiguration> {
        self.run("configuration", Value::Null, |_| {
            let mut config = SystemConfiguration::default();
            config.notifications.webhook = true;
            config.misc.sub_public_domain = "sub.example.com".into();
            Ok(config)
        })
    }

    async fn stats(&self) -> RemnaResult<SystemStats> {
        self.run("stats", Value::Null, |state| {
            let mut stats = SystemStats::default();
            for user in state.users.values() {
                let key = serde_json::to_value(user.status)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                *stats.users.status_counts.entry(key).or_default() += 1;
            }
            stats.users.total_users = state.users.len() as i64;
            Ok(stats)
        })
    }

    async fn get_user(&self, id: RemnaUserId) -> RemnaResult<RemnaUser> {
        self.run("get_user", json!({ "id": id }), |state| {
            state
                .users
                .get(&id.0)
                .cloned()
                .ok_or_else(|| not_found("User"))
        })
    }

    async fn get_user_by_username(&self, username: &str) -> RemnaResult<RemnaUser> {
        self.run(
            "get_user_by_username",
            json!({ "username": username }),
            |state| {
                state
                    .users
                    .values()
                    .find(|u| u.username == username)
                    .cloned()
                    .ok_or_else(|| not_found("User"))
            },
        )
    }

    async fn get_user_by_short_uuid(&self, short_uuid: &str) -> RemnaResult<RemnaUser> {
        self.run(
            "get_user_by_short_uuid",
            json!({ "shortUuid": short_uuid }),
            |state| {
                state
                    .users
                    .values()
                    .find(|u| u.short_uuid == short_uuid)
                    .cloned()
                    .ok_or_else(|| not_found("User"))
            },
        )
    }

    async fn stream_users(&self, query: &UsersStreamQuery) -> RemnaResult<UsersPage> {
        self.run("stream_users", args(query), |state| {
            let start: usize = query
                .cursor
                .as_deref()
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);
            let size = query.size.unwrap_or(250) as usize;
            let matching: Vec<RemnaUser> = state
                .users
                .values()
                .filter(|u| query.status.is_none_or(|s| u.status == s))
                .filter(|u| {
                    query
                        .telegram_id
                        .as_deref()
                        .is_none_or(|t| u.telegram_id.map(|v| v.to_string()).as_deref() == Some(t))
                })
                .filter(|u| {
                    query
                        .email
                        .as_deref()
                        .is_none_or(|e| u.email.as_deref() == Some(e))
                })
                .filter(|u| {
                    query
                        .tag
                        .as_deref()
                        .is_none_or(|t| u.tag.as_deref() == Some(t))
                })
                .cloned()
                .collect();
            let page: Vec<RemnaUser> = matching.iter().skip(start).take(size).cloned().collect();
            let next = start + page.len();
            Ok(UsersPage {
                users: page,
                next_cursor: (next < matching.len()).then(|| next.to_string()),
                has_more: next < matching.len(),
            })
        })
    }

    async fn create_user(&self, request: &CreateUserRequest) -> RemnaResult<RemnaUser> {
        let (applied, result, hook) = {
            let mut state = self.lock();
            state.calls.push(FakeCall {
                method: "create_user".into(),
                args: args(request),
            });
            let fault = state.take_fault("create_user");
            let (applied, result) = match fault {
                Some(fault) if !fault.applies() => (None, Err(fault.error())),
                fault => {
                    let created = Self::create_user_now(&mut state, request);
                    let applied = created.as_ref().ok().cloned();
                    match fault {
                        Some(fault) => (applied, Err(fault.error())),
                        None => (applied, created),
                    }
                }
            };
            (applied, result, state.create_hook.clone())
        };
        if let (Some(user), Some(hook)) = (applied, hook) {
            hook(user).await;
        }
        result
    }

    async fn update_user(&self, request: &UpdateUserRequest) -> RemnaResult<RemnaUser> {
        self.with_user("update_user", args(request), request.id, |user| {
            if let Some(status) = request.status {
                if matches!(status, UserStatus::Limited | UserStatus::Expired) {
                    return Err(RemnaError::BadRequest {
                        code: Some("A089".into()),
                        message: "LIMITED and EXPIRED statuses are not allowed to be set manually."
                            .into(),
                    });
                }
                user.status = status;
            }
            if let Some(v) = request.traffic_limit_bytes {
                user.traffic_limit_bytes = v;
            }
            if let Some(v) = request.traffic_limit_strategy {
                user.traffic_limit_strategy = v;
            }
            if let Some(v) = request.expire_at {
                user.expire_at = panel_time(v);
                if user.status == UserStatus::Expired && v > Utc::now() {
                    user.status = UserStatus::Active;
                }
            }
            if let Some(v) = &request.description {
                user.description = v.clone();
            }
            if let Some(v) = &request.tag {
                user.tag = v.clone();
            }
            if let Some(v) = request.telegram_id {
                user.telegram_id = v;
            }
            if let Some(v) = &request.email {
                user.email = v.clone();
            }
            if let Some(v) = request.hwid_device_limit {
                user.hwid_device_limit = v;
            }
            if let Some(v) = &request.active_internal_squads {
                user.active_internal_squads = squad_refs(v);
            }
            if let Some(v) = request.external_squad_uuid {
                user.external_squad_uuid = v;
            }
            Ok(user.clone())
        })
    }

    async fn delete_user(&self, id: RemnaUserId) -> RemnaResult<()> {
        self.run("delete_user", json!({ "id": id }), |state| {
            state.devices.remove(&id.0);
            state
                .users
                .remove(&id.0)
                .map(|_| ())
                .ok_or_else(|| not_found("User"))
        })
    }

    async fn extend_user(&self, id: RemnaUserId, days: u32) -> RemnaResult<RemnaUser> {
        self.with_user(
            "extend_user",
            json!({ "id": id, "days": days }),
            id,
            |user| {
                let now = Utc::now();
                if user.status == UserStatus::Expired || user.expire_at < now {
                    user.expire_at = panel_time(now + Duration::days(days as i64));
                    if user.status == UserStatus::Expired {
                        user.status = UserStatus::Active;
                    }
                } else {
                    user.expire_at = panel_time(user.expire_at + Duration::days(days as i64));
                }
                Ok(user.clone())
            },
        )
    }

    async fn reset_user_traffic(&self, id: RemnaUserId) -> RemnaResult<RemnaUser> {
        self.with_user("reset_user_traffic", json!({ "id": id }), id, |user| {
            user.user_traffic.used_traffic_bytes = 0;
            user.last_traffic_reset_at = Some(Utc::now());
            if user.status == UserStatus::Limited {
                user.status = UserStatus::Active;
            }
            Ok(user.clone())
        })
    }

    async fn disable_user(&self, id: RemnaUserId) -> RemnaResult<RemnaUser> {
        self.with_user("disable_user", json!({ "id": id }), id, |user| {
            if user.status == UserStatus::Disabled {
                return Err(RemnaError::BadRequest {
                    code: Some("A029".into()),
                    message: "User already disabled".into(),
                });
            }
            user.status = UserStatus::Disabled;
            Ok(user.clone())
        })
    }

    async fn enable_user(&self, id: RemnaUserId) -> RemnaResult<RemnaUser> {
        self.with_user("enable_user", json!({ "id": id }), id, |user| {
            if user.status == UserStatus::Active {
                return Err(RemnaError::BadRequest {
                    code: Some("A030".into()),
                    message: "User already enabled".into(),
                });
            }
            user.status = if user.expire_at < Utc::now() {
                UserStatus::Expired
            } else {
                UserStatus::Active
            };
            Ok(user.clone())
        })
    }

    async fn revoke_user(
        &self,
        id: RemnaUserId,
        request: &RevokeRequest,
    ) -> RemnaResult<RemnaUser> {
        self.with_user(
            "revoke_user",
            json!({ "id": id, "request": args(request) }),
            id,
            |user| {
                if !request.revoke_only_passwords.unwrap_or(false) {
                    user.short_uuid = request.short_uuid.clone().unwrap_or_else(new_short_uuid);
                    user.subscription_url = format!("https://sub.example.com/{}", user.short_uuid);
                }
                user.sub_revoked_at = Some(Utc::now());
                Ok(user.clone())
            },
        )
    }

    async fn user_accessible_nodes(&self, id: RemnaUserId) -> RemnaResult<UserAccessibleNodes> {
        self.run("user_accessible_nodes", json!({ "id": id }), |state| {
            if !state.users.contains_key(&id.0) {
                return Err(not_found("User"));
            }
            Ok(UserAccessibleNodes {
                user_id: id,
                active_nodes: state
                    .nodes
                    .iter()
                    .map(|n| AccessibleNode {
                        uuid: n.node.uuid,
                        node_name: n.node.name.clone(),
                        country_code: n.node.country_code.clone(),
                        config_profile_uuid: None,
                        config_profile_name: None,
                    })
                    .collect(),
            })
        })
    }

    async fn user_subscription_requests(
        &self,
        id: RemnaUserId,
    ) -> RemnaResult<SubscriptionRequests> {
        self.run("user_subscription_requests", json!({ "id": id }), |_| {
            Ok(SubscriptionRequests::default())
        })
    }

    async fn user_usage(&self, id: RemnaUserId, range: DateRange) -> RemnaResult<UserUsage> {
        self.run(
            "user_usage",
            json!({ "id": id, "range": args(&range) }),
            |_| Ok(UserUsage::default()),
        )
    }

    async fn user_devices(&self, id: RemnaUserId) -> RemnaResult<HwidDevices> {
        self.run("user_devices", json!({ "id": id }), |state| {
            Ok(Self::devices_of(state, id))
        })
    }

    async fn delete_user_device(&self, id: RemnaUserId, hwid: &str) -> RemnaResult<HwidDevices> {
        self.run(
            "delete_user_device",
            json!({ "userId": id, "hwid": hwid }),
            |state| {
                if let Some(list) = state.devices.get_mut(&id.0) {
                    list.retain(|d| d.hwid != hwid);
                }
                Ok(Self::devices_of(state, id))
            },
        )
    }

    async fn delete_all_user_devices(&self, id: RemnaUserId) -> RemnaResult<HwidDevices> {
        self.run(
            "delete_all_user_devices",
            json!({ "userId": id }),
            |state| {
                state.devices.remove(&id.0);
                Ok(HwidDevices::default())
            },
        )
    }

    async fn drop_connections(&self, request: &DropConnectionsRequest) -> RemnaResult<()> {
        self.run("drop_connections", args(request), |_| Ok(()))
    }

    async fn request_user_connections(&self, id: RemnaUserId) -> RemnaResult<ConnectionsJob> {
        self.run("request_user_connections", json!({ "id": id }), |_| {
            Ok(ConnectionsJob {
                job_id: format!("job-{id}"),
            })
        })
    }

    async fn user_connections(&self, job_id: &str) -> RemnaResult<UserConnectionsStatus> {
        self.run("user_connections", json!({ "jobId": job_id }), |_| {
            let user_id = job_id
                .strip_prefix("job-")
                .and_then(|v| v.parse().ok())
                .map(RemnaUserId)
                .ok_or_else(|| not_found("Job"))?;
            Ok(UserConnectionsStatus {
                is_completed: true,
                is_failed: false,
                progress: JobProgress {
                    total: 1,
                    completed: 1,
                },
                result: Some(UserConnectionsResult {
                    success: true,
                    user_id,
                    nodes: Vec::new(),
                }),
            })
        })
    }

    async fn bulk_update_users(&self, request: &BulkUpdateRequest) -> RemnaResult<()> {
        self.run("bulk_update_users", args(request), |state| {
            for id in &request.user_ids {
                if let Some(user) = state.users.get_mut(&id.0) {
                    if let Some(status) = request.fields.status {
                        user.status = status;
                    }
                    if let Some(v) = request.fields.traffic_limit_bytes {
                        user.traffic_limit_bytes = v;
                    }
                    if let Some(v) = request.fields.traffic_limit_strategy {
                        user.traffic_limit_strategy = v;
                    }
                    if let Some(v) = request.fields.expire_at {
                        user.expire_at = panel_time(v);
                    }
                    if let Some(v) = request.fields.hwid_device_limit {
                        user.hwid_device_limit = v;
                    }
                    if let Some(v) = &request.fields.tag {
                        user.tag = v.clone();
                    }
                }
            }
            Ok(())
        })
    }

    async fn bulk_update_squads(&self, request: &BulkSquadsRequest) -> RemnaResult<()> {
        self.run("bulk_update_squads", args(request), |state| {
            for id in &request.user_ids {
                if let Some(user) = state.users.get_mut(&id.0) {
                    user.active_internal_squads = squad_refs(&request.active_internal_squads);
                }
            }
            Ok(())
        })
    }

    async fn bulk_extend_users(&self, request: &BulkExtendRequest) -> RemnaResult<()> {
        self.run("bulk_extend_users", args(request), |state| {
            for id in &request.user_ids {
                if let Some(user) = state.users.get_mut(&id.0) {
                    user.expire_at =
                        panel_time(user.expire_at + Duration::days(request.extend_days as i64));
                }
            }
            Ok(())
        })
    }

    async fn bulk_reset_traffic(&self, request: &BulkUsersRequest) -> RemnaResult<()> {
        self.run("bulk_reset_traffic", args(request), |state| {
            for id in &request.user_ids {
                if let Some(user) = state.users.get_mut(&id.0) {
                    user.user_traffic.used_traffic_bytes = 0;
                    if user.status == UserStatus::Limited {
                        user.status = UserStatus::Active;
                    }
                }
            }
            Ok(())
        })
    }

    async fn nodes(&self) -> RemnaResult<Vec<Node>> {
        self.run("nodes", Value::Null, |state| {
            Ok(state.nodes.iter().map(|n| n.node.clone()).collect())
        })
    }

    async fn restart_node(&self, uuid: Uuid) -> RemnaResult<()> {
        self.run("restart_node", json!({ "uuid": uuid }), |state| {
            let node = state.node_mut(uuid)?;
            node.node.is_connecting = true;
            Ok(())
        })
    }

    async fn enable_node(&self, uuid: Uuid) -> RemnaResult<Node> {
        self.run("enable_node", json!({ "uuid": uuid }), |state| {
            let node = state.node_mut(uuid)?;
            node.node.is_disabled = false;
            Ok(node.node.clone())
        })
    }

    async fn disable_node(&self, uuid: Uuid) -> RemnaResult<Node> {
        self.run("disable_node", json!({ "uuid": uuid }), |state| {
            let node = state.node_mut(uuid)?;
            node.node.is_disabled = true;
            node.node.is_connected = false;
            Ok(node.node.clone())
        })
    }

    async fn internal_squads(&self) -> RemnaResult<InternalSquads> {
        self.run("internal_squads", Value::Null, |state| {
            Ok(InternalSquads {
                total: state.squads.len() as i64,
                internal_squads: state.squads.clone(),
            })
        })
    }

    async fn squad_accessible_nodes(&self, uuid: Uuid) -> RemnaResult<SquadAccessibleNodes> {
        self.run("squad_accessible_nodes", json!({ "uuid": uuid }), |state| {
            let squad = state
                .squads
                .iter()
                .find(|s| s.uuid == uuid)
                .ok_or_else(|| not_found("Internal squad"))?;
            let inbounds: BTreeSet<Uuid> = squad.inbounds.iter().map(|i| i.uuid).collect();
            Ok(SquadAccessibleNodes {
                squad_uuid: uuid,
                accessible_nodes: state
                    .nodes
                    .iter()
                    .filter(|n| {
                        n.config_profile.as_ref().is_some_and(|p| {
                            p.active_inbounds.iter().any(|i| inbounds.contains(&i.uuid))
                        })
                    })
                    .map(|n| AccessibleNode {
                        uuid: n.node.uuid,
                        node_name: n.node.name.clone(),
                        country_code: n.node.country_code.clone(),
                        config_profile_uuid: n
                            .config_profile
                            .as_ref()
                            .and_then(|p| p.active_config_profile_uuid),
                        config_profile_name: None,
                    })
                    .collect(),
            })
        })
    }

    async fn subscription_settings(&self) -> RemnaResult<SubscriptionSettings> {
        self.run("subscription_settings", Value::Null, |state| {
            state
                .settings
                .clone()
                .ok_or_else(|| not_found("Subscription settings"))
        })
    }

    async fn update_subscription_settings(
        &self,
        request: &UpdateSubscriptionSettingsRequest,
    ) -> RemnaResult<SubscriptionSettings> {
        self.run("update_subscription_settings", args(request), |state| {
            let settings = state
                .settings
                .as_mut()
                .filter(|s| s.uuid == request.uuid)
                .ok_or_else(|| not_found("Subscription settings"))?;
            if let Some(v) = request.is_show_custom_remarks {
                settings.is_show_custom_remarks = v;
            }
            if let Some(v) = &request.custom_remarks {
                settings.custom_remarks = v.clone();
            }
            if let Some(v) = &request.hwid_settings {
                settings.hwid_settings = Some(v.clone());
            }
            Ok(settings.clone())
        })
    }

    async fn infra_billing_nodes(&self) -> RemnaResult<InfraBillingNodes> {
        self.run("infra_billing_nodes", Value::Null, |state| {
            Ok(state.billing())
        })
    }

    async fn node(&self, uuid: Uuid) -> RemnaResult<NodeDetails> {
        self.run("node", json!({ "uuid": uuid }), |state| {
            state.node_mut(uuid).map(|n| n.clone())
        })
    }

    async fn nodes_detailed(&self) -> RemnaResult<Vec<NodeDetails>> {
        self.run("nodes_detailed", Value::Null, |state| {
            Ok(state.nodes.clone())
        })
    }

    async fn create_node(&self, request: &CreateNodeRequest) -> RemnaResult<NodeDetails> {
        self.run("create_node", args(request), |state| {
            let provider = request
                .provider_uuid
                .and_then(|uuid| state.providers.iter().find(|p| p.uuid == uuid))
                .map(|p| NodeProvider {
                    uuid: p.uuid,
                    name: p.name.clone(),
                    login_url: p.login_url.clone(),
                });
            let mut node = sample_node(
                Uuid::new_v4(),
                &request.name,
                request.country_code.as_deref().unwrap_or("XX"),
            );
            node.address = request.address.clone();
            node.port = request.port;
            node.is_connected = false;
            node.provider = provider;
            node.traffic_limit_bytes = request.traffic_limit_bytes;
            node.notify_percent = request.notify_percent;
            node.traffic_reset_day = request.traffic_reset_day;
            node.consumption_multiplier = request.consumption_multiplier.unwrap_or(1.0);
            node.tags = request.tags.clone();
            node.note = request.note.clone();
            let details = NodeDetails {
                node,
                provider_uuid: request.provider_uuid,
                is_traffic_tracking_active: request.is_traffic_tracking_active.unwrap_or(false),
                view_position: state.nodes.len() as i64,
                config_profile: Some(NodeConfigProfile {
                    active_config_profile_uuid: Some(
                        request.config_profile.active_config_profile_uuid,
                    ),
                    active_inbounds: state.inbounds(&request.config_profile.active_inbounds),
                }),
                system: None,
                created_at: Some(Utc::now()),
            };
            state.nodes.push(details.clone());
            Ok(details)
        })
    }

    async fn update_node(&self, request: &UpdateNodeRequest) -> RemnaResult<NodeDetails> {
        self.run("update_node", args(request), |state| {
            let inbounds = request
                .config_profile
                .as_ref()
                .map(|p| state.inbounds(&p.active_inbounds));
            let details = state.node_mut(request.uuid)?;
            let node = &mut details.node;
            if let Some(v) = &request.name {
                node.name = v.clone();
            }
            if let Some(v) = &request.address {
                node.address = v.clone();
            }
            if let Some(v) = request.port {
                node.port = Some(v);
            }
            if let Some(v) = &request.country_code {
                node.country_code = v.clone();
            }
            if let Some(v) = request.traffic_limit_bytes {
                node.traffic_limit_bytes = Some(v);
            }
            if let Some(v) = request.notify_percent {
                node.notify_percent = Some(v);
            }
            if let Some(v) = request.traffic_reset_day {
                node.traffic_reset_day = Some(v);
            }
            if let Some(v) = request.consumption_multiplier {
                node.consumption_multiplier = v;
            }
            if let Some(v) = &request.tags {
                node.tags = v.clone();
            }
            if let Some(v) = &request.note {
                node.note = v.clone();
            }
            if let Some(v) = request.is_traffic_tracking_active {
                details.is_traffic_tracking_active = v;
            }
            if let Some(v) = request.provider_uuid {
                details.provider_uuid = v;
            }
            if let (Some(profile), Some(inbounds)) = (&request.config_profile, inbounds) {
                details.config_profile = Some(NodeConfigProfile {
                    active_config_profile_uuid: Some(profile.active_config_profile_uuid),
                    active_inbounds: inbounds,
                });
            }
            Ok(details.clone())
        })
    }

    async fn delete_node(&self, uuid: Uuid) -> RemnaResult<()> {
        self.run("delete_node", json!({ "uuid": uuid }), |state| {
            let before = state.nodes.len();
            state.nodes.retain(|n| n.node.uuid != uuid);
            if state.nodes.len() == before {
                return Err(not_found("Node"));
            }
            state.metadata.remove(&uuid);
            Ok(())
        })
    }

    async fn reset_node_traffic(&self, uuid: Uuid) -> RemnaResult<()> {
        self.run("reset_node_traffic", json!({ "uuid": uuid }), |state| {
            let node = state.node_mut(uuid)?;
            node.node.traffic_used_bytes = Some(0);
            Ok(())
        })
    }

    async fn restart_all_nodes(&self, force: bool) -> RemnaResult<()> {
        self.run(
            "restart_all_nodes",
            json!({ "forceRestart": force }),
            |state| {
                for node in state.nodes.iter_mut().filter(|n| !n.node.is_disabled) {
                    node.node.is_connecting = true;
                }
                Ok(())
            },
        )
    }

    async fn keygen(&self) -> RemnaResult<NodeKeygen> {
        self.run("keygen", Value::Null, |_| {
            Ok(NodeKeygen {
                secret_key: "fake-node-secret-key".into(),
            })
        })
    }

    async fn config_profiles(&self) -> RemnaResult<ConfigProfiles> {
        self.run("config_profiles", Value::Null, |state| {
            Ok(ConfigProfiles {
                total: state.config_profiles.len() as i64,
                config_profiles: state.config_profiles.clone(),
            })
        })
    }

    async fn hosts(&self) -> RemnaResult<Vec<Host>> {
        self.run("hosts", Value::Null, |state| Ok(state.hosts.clone()))
    }

    async fn update_host(&self, request: &UpdateHostRequest) -> RemnaResult<Host> {
        self.run("update_host", args(request), |state| {
            let host = state
                .hosts
                .iter_mut()
                .find(|h| h.uuid == request.uuid)
                .ok_or_else(|| not_found("Host"))?;
            if let Some(v) = &request.remark {
                host.remark = v.clone();
            }
            if let Some(v) = &request.address {
                host.address = v.clone();
            }
            if let Some(v) = request.port {
                host.port = v;
            }
            if let Some(v) = &request.sni {
                host.sni = v.clone();
            }
            if let Some(v) = &request.host {
                host.host = v.clone();
            }
            if let Some(v) = &request.path {
                host.path = v.clone();
            }
            if let Some(v) = &request.alpn {
                host.alpn = v.clone();
            }
            if let Some(v) = &request.fingerprint {
                host.fingerprint = v.clone();
            }
            if let Some(v) = &request.security_layer {
                host.security_layer = Some(v.clone());
            }
            if let Some(v) = request.is_disabled {
                host.is_disabled = v;
            }
            if let Some(v) = request.is_hidden {
                host.is_hidden = v;
            }
            if let Some(v) = &request.internal_squads {
                host.internal_squads = Some(v.clone());
            }
            Ok(host.clone())
        })
    }

    async fn create_host(&self, request: &CreateHostRequest) -> RemnaResult<Host> {
        self.run("create_host", args(request), |state| {
            let target = request.inbound;
            let known = state.config_profiles.iter().any(|p| {
                p.uuid == target.config_profile_uuid
                    && p.inbounds
                        .iter()
                        .any(|i| i.uuid == target.config_profile_inbound_uuid)
            });
            if !known {
                return Err(not_found("Config profile inbound"));
            }
            let host = Host {
                uuid: Uuid::new_v4(),
                view_position: state
                    .hosts
                    .iter()
                    .map(|h| h.view_position + 1)
                    .max()
                    .unwrap_or(0),
                remark: request.remark.clone(),
                address: request.address.clone(),
                port: request.port,
                is_disabled: request.is_disabled.unwrap_or(false),
                inbound: HostInbound {
                    config_profile_uuid: Some(target.config_profile_uuid),
                    config_profile_inbound_uuid: Some(target.config_profile_inbound_uuid),
                },
                internal_squads: Some(HostSquadFilter::exclude(Vec::new())),
                sni: request.sni.clone(),
                host: request.host.clone(),
                path: request.path.clone(),
                alpn: request.alpn.clone(),
                fingerprint: request.fingerprint.clone(),
                security_layer: Some(
                    request
                        .security_layer
                        .clone()
                        .unwrap_or_else(|| "DEFAULT".into()),
                ),
                ..Host::default()
            };
            state.hosts.push(host.clone());
            Ok(host)
        })
    }

    async fn delete_host(&self, uuid: Uuid) -> RemnaResult<()> {
        self.run("delete_host", json!({ "uuid": uuid }), |state| {
            let before = state.hosts.len();
            state.hosts.retain(|h| h.uuid != uuid);
            if state.hosts.len() == before {
                return Err(not_found("Host"));
            }
            Ok(())
        })
    }

    async fn reorder_hosts(&self, positions: &[HostPosition]) -> RemnaResult<()> {
        self.run("reorder_hosts", json!({ "hosts": positions }), |state| {
            if positions
                .iter()
                .any(|p| !state.hosts.iter().any(|h| h.uuid == p.uuid))
            {
                return Err(not_found("Host"));
            }
            for position in positions {
                if let Some(host) = state.hosts.iter_mut().find(|h| h.uuid == position.uuid) {
                    host.view_position = position.view_position;
                }
            }
            Ok(())
        })
    }

    async fn enable_hosts(&self, uuids: &[Uuid]) -> RemnaResult<()> {
        self.run("enable_hosts", json!({ "uuids": uuids }), |state| {
            for host in state.hosts.iter_mut().filter(|h| uuids.contains(&h.uuid)) {
                host.is_disabled = false;
            }
            Ok(())
        })
    }

    async fn disable_hosts(&self, uuids: &[Uuid]) -> RemnaResult<()> {
        self.run("disable_hosts", json!({ "uuids": uuids }), |state| {
            for host in state.hosts.iter_mut().filter(|h| uuids.contains(&h.uuid)) {
                host.is_disabled = true;
            }
            Ok(())
        })
    }

    async fn create_internal_squad(
        &self,
        request: &CreateInternalSquadRequest,
    ) -> RemnaResult<InternalSquad> {
        self.run("create_internal_squad", args(request), |state| {
            let inbounds = state.inbounds(&request.inbounds);
            let squad = InternalSquad {
                uuid: Uuid::new_v4(),
                name: request.name.clone(),
                view_position: state.squads.len() as i64,
                info: SquadInfo {
                    members_count: 0,
                    inbounds_count: inbounds.len() as i64,
                },
                inbounds,
            };
            state.squads.push(squad.clone());
            Ok(squad)
        })
    }

    async fn update_internal_squad(
        &self,
        request: &UpdateInternalSquadRequest,
    ) -> RemnaResult<InternalSquad> {
        self.run("update_internal_squad", args(request), |state| {
            let inbounds = request.inbounds.as_ref().map(|i| state.inbounds(i));
            let squad = state
                .squads
                .iter_mut()
                .find(|s| s.uuid == request.uuid)
                .ok_or_else(|| not_found("Internal squad"))?;
            if let Some(name) = &request.name {
                squad.name = name.clone();
            }
            if let Some(inbounds) = inbounds {
                squad.info.inbounds_count = inbounds.len() as i64;
                squad.inbounds = inbounds;
            }
            Ok(squad.clone())
        })
    }

    async fn delete_internal_squad(&self, uuid: Uuid) -> RemnaResult<()> {
        self.run("delete_internal_squad", json!({ "uuid": uuid }), |state| {
            let before = state.squads.len();
            state.squads.retain(|s| s.uuid != uuid);
            if state.squads.len() == before {
                return Err(not_found("Internal squad"));
            }
            Ok(())
        })
    }

    async fn internal_squad_usage(
        &self,
        uuid: Uuid,
        query: &SquadUsageQuery,
    ) -> RemnaResult<SquadUsage> {
        self.run(
            "internal_squad_usage",
            json!({ "uuid": uuid, "query": args(query) }),
            |state| {
                if !state.squads.iter().any(|s| s.uuid == uuid) {
                    return Err(not_found("Internal squad"));
                }
                let users = state
                    .users
                    .values()
                    .filter(|u| u.active_internal_squads.iter().any(|s| s.uuid == uuid))
                    .map(|u| SquadUserUsage {
                        id: u.id,
                        total_bytes: u.user_traffic.used_traffic_bytes,
                    })
                    .filter(|u| query.min_total_bytes.is_none_or(|min| u.total_bytes >= min))
                    .collect();
                Ok(SquadUsage {
                    squad_uuid: uuid,
                    users,
                    next_cursor: None,
                    has_more: false,
                })
            },
        )
    }

    async fn external_squads(&self) -> RemnaResult<ExternalSquads> {
        self.run("external_squads", Value::Null, |state| {
            Ok(ExternalSquads {
                total: state.external_squads.len() as i64,
                external_squads: state.external_squads.clone(),
            })
        })
    }

    async fn infra_providers(&self) -> RemnaResult<InfraProviders> {
        self.run("infra_providers", Value::Null, |state| {
            Ok(InfraProviders {
                total: state.providers.len() as i64,
                providers: state.providers.clone(),
            })
        })
    }

    async fn infra_provider(&self, uuid: Uuid) -> RemnaResult<InfraProvider> {
        self.run("infra_provider", json!({ "uuid": uuid }), |state| {
            state
                .providers
                .iter()
                .find(|p| p.uuid == uuid)
                .cloned()
                .ok_or_else(|| not_found("Infra provider"))
        })
    }

    async fn create_infra_provider(
        &self,
        request: &CreateInfraProviderRequest,
    ) -> RemnaResult<InfraProvider> {
        self.run("create_infra_provider", args(request), |state| {
            let provider = InfraProvider {
                uuid: Uuid::new_v4(),
                name: request.name.clone(),
                favicon_link: request.favicon_link.clone(),
                login_url: request.login_url.clone(),
                billing_history: ProviderBillingSummary::default(),
                billing_nodes: Vec::new(),
            };
            state.providers.push(provider.clone());
            Ok(provider)
        })
    }

    async fn update_infra_provider(
        &self,
        request: &UpdateInfraProviderRequest,
    ) -> RemnaResult<InfraProvider> {
        self.run("update_infra_provider", args(request), |state| {
            let provider = state
                .providers
                .iter_mut()
                .find(|p| p.uuid == request.uuid)
                .ok_or_else(|| not_found("Infra provider"))?;
            if let Some(name) = &request.name {
                provider.name = name.clone();
            }
            if let Some(link) = &request.favicon_link {
                provider.favicon_link = link.clone();
            }
            if let Some(url) = &request.login_url {
                provider.login_url = url.clone();
            }
            Ok(provider.clone())
        })
    }

    async fn delete_infra_provider(&self, uuid: Uuid) -> RemnaResult<()> {
        self.run("delete_infra_provider", json!({ "uuid": uuid }), |state| {
            let before = state.providers.len();
            state.providers.retain(|p| p.uuid != uuid);
            if state.providers.len() == before {
                return Err(not_found("Infra provider"));
            }
            state.billing_nodes.retain(|n| n.provider_uuid != uuid);
            Ok(())
        })
    }

    async fn infra_billing_history(
        &self,
        query: &BillingHistoryQuery,
    ) -> RemnaResult<InfraBillingHistory> {
        self.run("infra_billing_history", args(query), |state| {
            let start = query.start.unwrap_or(0) as usize;
            let size = query.size.unwrap_or(50) as usize;
            Ok(InfraBillingHistory {
                records: state
                    .history
                    .iter()
                    .skip(start)
                    .take(size)
                    .cloned()
                    .collect(),
                total: state.history.len() as i64,
            })
        })
    }

    async fn create_infra_billing_record(
        &self,
        request: &CreateBillingRecordRequest,
    ) -> RemnaResult<InfraBillingHistory> {
        self.run("create_infra_billing_record", args(request), |state| {
            let provider = state
                .providers
                .iter_mut()
                .find(|p| p.uuid == request.provider_uuid)
                .ok_or_else(|| not_found("Infra provider"))?;
            provider.billing_history.total_bills += 1;
            provider.billing_history.total_amount = BillingAmount(
                provider.billing_history.total_amount.minor() + request.amount.minor(),
            );
            let record = BillingRecord {
                uuid: Uuid::new_v4(),
                provider_uuid: provider.uuid,
                amount: request.amount,
                billed_at: request.billed_at,
                provider: Some(BillingRecordProvider {
                    uuid: provider.uuid,
                    name: provider.name.clone(),
                }),
            };
            state.history.insert(0, record);
            Ok(state.history())
        })
    }

    async fn create_infra_billing_node(
        &self,
        request: &CreateBillingNodeRequest,
    ) -> RemnaResult<InfraBillingNodes> {
        self.run("create_infra_billing_node", args(request), |state| {
            let provider = state
                .providers
                .iter()
                .find(|p| p.uuid == request.provider_uuid)
                .map(|p| BillingProvider {
                    uuid: p.uuid,
                    name: p.name.clone(),
                    login_url: p.login_url.clone(),
                })
                .ok_or_else(|| not_found("Infra provider"))?;
            let node = match request.node_uuid {
                Some(uuid) => Some(
                    state
                        .nodes
                        .iter()
                        .find(|n| n.node.uuid == uuid)
                        .map(|n| BillingNodeRef {
                            uuid: n.node.uuid,
                            name: n.node.name.clone(),
                            country_code: n.node.country_code.clone(),
                        })
                        .ok_or_else(|| not_found("Node"))?,
                ),
                None => None,
            };
            state.billing_nodes.push(BillingNode {
                uuid: Uuid::new_v4(),
                node_uuid: request.node_uuid,
                name: request.name.clone(),
                provider_uuid: provider.uuid,
                provider,
                node,
                next_billing_at: request.next_billing_at,
            });
            Ok(state.billing())
        })
    }

    async fn update_infra_billing_nodes(
        &self,
        request: &UpdateBillingNodesRequest,
    ) -> RemnaResult<InfraBillingNodes> {
        self.run("update_infra_billing_nodes", args(request), |state| {
            for node in state
                .billing_nodes
                .iter_mut()
                .filter(|n| request.uuids.contains(&n.uuid))
            {
                node.next_billing_at = request.next_billing_at;
            }
            Ok(state.billing())
        })
    }

    async fn node_metadata(&self, uuid: Uuid) -> RemnaResult<NodeMetadata> {
        self.run("node_metadata", json!({ "uuid": uuid }), |state| {
            state.node_mut(uuid)?;
            Ok(state.metadata.get(&uuid).cloned().unwrap_or_default())
        })
    }

    async fn set_node_metadata(
        &self,
        uuid: Uuid,
        metadata: &NodeMetadata,
    ) -> RemnaResult<NodeMetadata> {
        self.run(
            "set_node_metadata",
            json!({ "uuid": uuid, "metadata": args(metadata) }),
            |state| {
                state.node_mut(uuid)?;
                state.metadata.insert(uuid, metadata.clone());
                Ok(metadata.clone())
            },
        )
    }

    async fn subscription_page_configs(&self) -> RemnaResult<SubpageConfigs> {
        self.run("subscription_page_configs", Value::Null, |state| {
            Ok(SubpageConfigs {
                total: state.subpage_configs.len() as i64,
                configs: state.subpage_configs.clone(),
            })
        })
    }

    async fn subscription_page_config(&self, uuid: Uuid) -> RemnaResult<SubpageConfig> {
        self.run(
            "subscription_page_config",
            json!({ "uuid": uuid }),
            |state| {
                let found = state
                    .subpage_configs
                    .iter()
                    .find(|config| config.uuid == uuid)
                    .ok_or_else(|| not_found("SubscriptionPageConfig"))?;
                Ok(SubpageConfig {
                    uuid,
                    name: found.name.clone(),
                    view_position: found.view_position,
                    config: state.subpage_contents.get(&uuid).cloned(),
                })
            },
        )
    }

    async fn subpage_access(&self, short_uuid: &str) -> RemnaResult<SubpageAccess> {
        self.run(
            "subpage_access",
            json!({ "shortUuid": short_uuid }),
            |state| {
                if !state.users.values().any(|u| u.short_uuid == short_uuid) {
                    return Err(not_found("User"));
                }
                Ok(SubpageAccess {
                    subpage_config_uuid: state.subpage_configs.first().map(|c| c.uuid),
                    webpage_allowed: true,
                })
            },
        )
    }

    async fn request_node_connections(&self, uuid: Uuid) -> RemnaResult<ConnectionsJob> {
        self.run(
            "request_node_connections",
            json!({ "uuid": uuid }),
            |state| {
                state.node_mut(uuid)?;
                Ok(ConnectionsJob {
                    job_id: format!("node-{uuid}"),
                })
            },
        )
    }

    async fn node_connections(&self, job_id: &str) -> RemnaResult<NodeConnectionsStatus> {
        self.run("node_connections", json!({ "jobId": job_id }), |_| {
            let node_uuid = job_id
                .strip_prefix("node-")
                .and_then(|v| Uuid::parse_str(v).ok())
                .ok_or_else(|| not_found("Job"))?;
            Ok(NodeConnectionsStatus {
                is_completed: true,
                is_failed: false,
                result: Some(NodeConnectionsResult {
                    success: true,
                    node_uuid,
                    users: Vec::new(),
                }),
            })
        })
    }

    async fn nodes_usage(&self, query: &NodesUsageQuery) -> RemnaResult<NodesUsage> {
        self.run("nodes_usage", args(query), |_| Ok(NodesUsage::default()))
    }

    async fn bandwidth_stats(&self, tz: Option<&str>) -> RemnaResult<BandwidthStats> {
        self.run("bandwidth_stats", json!({ "tz": tz }), |_| {
            Ok(BandwidthStats::default())
        })
    }

    async fn nodes_metrics(&self) -> RemnaResult<NodesMetrics> {
        self.run("nodes_metrics", Value::Null, |state| {
            Ok(NodesMetrics {
                nodes: state
                    .nodes
                    .iter()
                    .map(|n| NodeMetrics {
                        node_uuid: n.node.uuid.to_string(),
                        node_name: n.node.name.clone(),
                        provider_name: n
                            .node
                            .provider
                            .as_ref()
                            .map(|p| p.name.clone())
                            .unwrap_or_default(),
                        users_online: n.node.users_online,
                        inbounds_stats: Vec::new(),
                        outbounds_stats: Vec::new(),
                    })
                    .collect(),
            })
        })
    }

    async fn torrent_blocker_reports(
        &self,
        query: &TorrentReportsQuery,
    ) -> RemnaResult<TorrentReports> {
        self.run("torrent_blocker_reports", args(query), |state| {
            let start = query.start.unwrap_or(0) as usize;
            let size = query.size.unwrap_or(25) as usize;
            Ok(TorrentReports {
                records: state
                    .torrent_reports
                    .iter()
                    .skip(start)
                    .take(size)
                    .cloned()
                    .collect(),
                total: state.torrent_reports.len() as i64,
            })
        })
    }

    async fn torrent_blocker_stats(&self) -> RemnaResult<TorrentStats> {
        self.run("torrent_blocker_stats", Value::Null, |state| {
            let users: BTreeSet<i64> = state.torrent_reports.iter().map(|r| r.user_id.0).collect();
            let nodes: BTreeSet<Uuid> = state.torrent_reports.iter().map(|r| r.node.uuid).collect();
            Ok(TorrentStats {
                stats: TorrentStatsSummary {
                    distinct_nodes: nodes.len() as i64,
                    distinct_users: users.len() as i64,
                    total_reports: state.torrent_reports.len() as i64,
                    reports_last_24_hours: state
                        .torrent_reports
                        .iter()
                        .filter(|r| r.created_at > Utc::now() - Duration::hours(24))
                        .count() as i64,
                },
                top_users: Vec::new(),
                top_nodes: Vec::new(),
            })
        })
    }

    async fn execute_node_plugin(&self, request: &PluginExecutorRequest) -> RemnaResult<()> {
        self.run("execute_node_plugin", args(request), |state| {
            match &request.command {
                PluginCommand::UnblockIps { ips } => {
                    state
                        .torrent_reports
                        .retain(|r| !ips.contains(&r.report.action_report.ip));
                    state.unblocked_ips.extend(ips.iter().cloned());
                }
            }
            Ok(())
        })
    }

    async fn bulk_delete_users(&self, request: &BulkUsersRequest) -> RemnaResult<()> {
        self.run("bulk_delete_users", args(request), |state| {
            for id in &request.user_ids {
                state.users.remove(&id.0);
                state.devices.remove(&id.0);
            }
            Ok(())
        })
    }

    async fn bulk_revoke_subscriptions(&self, request: &BulkUsersRequest) -> RemnaResult<()> {
        self.run("bulk_revoke_subscriptions", args(request), |state| {
            for id in &request.user_ids {
                if let Some(user) = state.users.get_mut(&id.0) {
                    user.short_uuid = new_short_uuid();
                    user.subscription_url = format!("https://sub.example.com/{}", user.short_uuid);
                    user.sub_revoked_at = Some(Utc::now());
                    user.updated_at = Utc::now();
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn таймаут_после_применения_оставляет_изменение() {
        let fake = FakeRemnawave::new();
        fake.insert_user(sample_user(1, "sn_a"));
        fake.fail_next(Fault::TimeoutAfterApply);
        let mut patch = UpdateUserRequest::new(RemnaUserId(1));
        patch.hwid_device_limit = Some(Some(5));
        assert_eq!(fake.update_user(&patch).await, Err(RemnaError::Timeout));
        assert_eq!(
            fake.user(RemnaUserId(1)).unwrap().hwid_device_limit,
            Some(5)
        );
    }

    fn create_request(username: &str) -> CreateUserRequest {
        CreateUserRequest {
            username: username.into(),
            status: None,
            expire_at: Utc::now(),
            traffic_limit_bytes: 0,
            traffic_limit_strategy: TrafficStrategy::NoReset,
            description: None,
            tag: None,
            telegram_id: Some(1),
            email: None,
            hwid_device_limit: Some(3),
            active_internal_squads: vec![Uuid::nil()],
            external_squad_uuid: None,
        }
    }

    fn with_microseconds(days: i64) -> DateTime<Utc> {
        panel_time(Utc::now() + Duration::days(days)) + Duration::microseconds(7)
    }

    #[tokio::test]
    async fn срок_хранится_с_точностью_до_миллисекунд_как_в_панели() {
        let fake = FakeRemnawave::new();
        let mut request = create_request("sn_ms");
        request.expire_at = with_microseconds(30);
        let created = fake.create_user(&request).await.unwrap();
        assert_eq!(created.expire_at, panel_time(request.expire_at));
        assert_ne!(created.expire_at, request.expire_at);
        assert_eq!(fake.user(created.id).unwrap().expire_at, created.expire_at);

        let target = with_microseconds(60);
        let mut patch = UpdateUserRequest::new(created.id);
        patch.expire_at = Some(target);
        let updated = fake.update_user(&patch).await.unwrap();
        assert_eq!(updated.expire_at, panel_time(target));
        assert_eq!(
            fake.get_user(created.id).await.unwrap().expire_at,
            panel_time(target)
        );

        fake.fail_on("update_user", Fault::TimeoutAfterApply);
        let lost = with_microseconds(90);
        patch.expire_at = Some(lost);
        assert_eq!(fake.update_user(&patch).await, Err(RemnaError::Timeout));
        assert_eq!(fake.user(created.id).unwrap().expire_at, panel_time(lost));

        let extended = fake.extend_user(created.id, 7).await.unwrap();
        assert_eq!(extended.expire_at, panel_time(lost) + Duration::days(7));

        let mut expired = sample_user(40, "sn_ms_expired");
        expired.status = UserStatus::Expired;
        expired.expire_at = with_microseconds(-3);
        let raw = expired.expire_at;
        fake.insert_user(expired);
        assert_eq!(
            fake.user(RemnaUserId(40)).unwrap().expire_at,
            panel_time(raw)
        );
        let revived = fake.extend_user(RemnaUserId(40), 30).await.unwrap();
        assert_eq!(revived.expire_at, panel_time(revived.expire_at));
        assert_eq!(revived.status, UserStatus::Active);

        let sample = sample_user(41, "sn_ms_sample");
        assert_eq!(sample.expire_at, panel_time(sample.expire_at));
    }

    #[tokio::test]
    async fn имя_пользователя_уникально() {
        let fake = FakeRemnawave::new();
        let request = create_request("sn_b");
        assert!(fake.create_user(&request).await.is_ok());
        let err = fake.create_user(&request).await.unwrap_err();
        assert_eq!(err.code(), Some("A019"));
        assert_eq!(
            fake.get_user_by_username("sn_b").await.unwrap().id,
            RemnaUserId(1)
        );
    }

    #[tokio::test]
    async fn вебхук_о_создании_приходит_раньше_ответа_на_post() {
        let fake = Arc::new(FakeRemnawave::new());
        let seen: Arc<Mutex<Vec<(String, bool)>>> = Arc::new(Mutex::new(Vec::new()));
        let observer = fake.clone();
        let log = seen.clone();
        fake.before_create_response(move |user| {
            let observer = observer.clone();
            let log = log.clone();
            Box::pin(async move {
                let body = user_webhook("user.created", &user, Utc::now());
                let parsed = crate::webhook::parse(&body).unwrap();
                let exists = observer.get_user(user.id).await.is_ok();
                if let Ok(mut log) = log.lock() {
                    log.push((parsed.event, exists));
                }
            })
        });
        let created = fake.create_user(&create_request("sn_race")).await.unwrap();
        assert_eq!(created.username, "sn_race");
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen, vec![("user.created".to_string(), true)]);

        fake.fail_on("create_user", Fault::TimeoutAfterApply);
        assert_eq!(
            fake.create_user(&create_request("sn_lost")).await,
            Err(RemnaError::Timeout)
        );
        assert!(fake.get_user_by_username("sn_lost").await.is_ok());
    }

    #[tokio::test]
    async fn повторное_отключение_и_включение_отвечают_ошибками_как_панель() {
        let fake = FakeRemnawave::new();
        fake.insert_user(sample_user(8, "sn_twice"));
        let err = fake.enable_user(RemnaUserId(8)).await.unwrap_err();
        assert_eq!(err.code(), Some("A030"));
        assert!(!err.outcome_unknown());
        let disabled = fake.disable_user(RemnaUserId(8)).await.unwrap();
        assert_eq!(disabled.status, UserStatus::Disabled);
        let err = fake.disable_user(RemnaUserId(8)).await.unwrap_err();
        assert_eq!(err.code(), Some("A029"));
        assert!(!err.outcome_unknown());
        assert_eq!(
            fake.user(RemnaUserId(8)).unwrap().status,
            UserStatus::Disabled
        );
        assert_eq!(
            fake.enable_user(RemnaUserId(8)).await.unwrap().status,
            UserStatus::Active
        );

        let mut expired = sample_user(9, "sn_expired_twice");
        expired.status = UserStatus::Disabled;
        expired.expire_at = Utc::now() - Duration::days(1);
        fake.insert_user(expired);
        assert_eq!(
            fake.enable_user(RemnaUserId(9)).await.unwrap().status,
            UserStatus::Expired
        );
        assert_eq!(
            fake.disable_user(RemnaUserId(9)).await.unwrap().status,
            UserStatus::Disabled
        );
    }

    #[tokio::test]
    async fn вызовы_учитываются_с_аргументами_и_сбой_по_имени_метода() {
        let fake = FakeRemnawave::new();
        fake.insert_user(sample_user(7, "sn_c"));
        fake.fail_on("extend_user", Fault::ServerError);
        assert!(fake.get_user(RemnaUserId(7)).await.is_ok());
        assert!(fake.extend_user(RemnaUserId(7), 3).await.is_err());
        assert!(fake.extend_user(RemnaUserId(7), 3).await.is_ok());
        fake.drop_connections(&DropConnectionsRequest::users_everywhere(vec![
            RemnaUserId(7),
        ]))
        .await
        .unwrap();
        assert_eq!(
            fake.calls(),
            vec!["get_user", "extend_user", "extend_user", "drop_connections"]
        );
        assert_eq!(fake.calls_of("extend_user")[0], json!({"id": 7, "days": 3}));
        assert_eq!(
            fake.calls_of("drop_connections")[0]["dropBy"]["userIds"],
            json!([7])
        );
        assert_eq!(fake.call_log().len(), 4);
    }

    #[tokio::test]
    async fn инфраструктура_в_подделке_меняется_как_в_панели() {
        let fake = FakeRemnawave::new();
        let profile = Uuid::new_v4();
        let inbound = Inbound {
            uuid: Uuid::new_v4(),
            profile_uuid: Some(profile),
            tag: "VLESS_REALITY".into(),
            kind: "vless".into(),
            network: None,
            security: Some("reality".into()),
            port: Some(443),
        };
        fake.set_config_profiles(vec![ConfigProfile {
            uuid: profile,
            name: "Основной профиль".into(),
            view_position: 0,
            inbounds: vec![inbound.clone()],
            nodes: Vec::new(),
        }]);
        let provider = fake
            .create_infra_provider(&CreateInfraProviderRequest {
                name: "Aeza".into(),
                favicon_link: None,
                login_url: None,
            })
            .await
            .unwrap();
        let mut request = CreateNodeRequest::new(
            "NL-1",
            "198.51.100.21",
            NodeProfileSelection {
                active_config_profile_uuid: profile,
                active_inbounds: vec![inbound.uuid],
            },
        );
        request.provider_uuid = Some(provider.uuid);
        let node = fake.create_node(&request).await.unwrap();
        assert_eq!(node.node.provider.as_ref().unwrap().name, "Aeza");
        let squad = fake
            .create_internal_squad(&CreateInternalSquadRequest {
                name: "Основной".into(),
                inbounds: vec![inbound.uuid],
            })
            .await
            .unwrap();
        let accessible = fake.squad_accessible_nodes(squad.uuid).await.unwrap();
        assert_eq!(accessible.accessible_nodes[0].node_name, "NL-1");
        let mut metadata = NodeMetadata::default();
        metadata.set_city(Some("Амстердам"));
        fake.set_node_metadata(node.node.uuid, &metadata)
            .await
            .unwrap();
        assert_eq!(
            fake.node_metadata(node.node.uuid).await.unwrap().city(),
            Some("Амстердам")
        );
        let history = fake
            .create_infra_billing_record(&CreateBillingRecordRequest {
                provider_uuid: provider.uuid,
                amount: BillingAmount(1400),
                billed_at: Utc::now(),
            })
            .await
            .unwrap();
        assert_eq!(history.total, 1);
        assert_eq!(
            fake.infra_provider(provider.uuid)
                .await
                .unwrap()
                .billing_history
                .total_amount,
            BillingAmount(1400)
        );
        fake.delete_node(node.node.uuid).await.unwrap();
        assert!(fake.node(node.node.uuid).await.unwrap_err().is_not_found());
    }
}
