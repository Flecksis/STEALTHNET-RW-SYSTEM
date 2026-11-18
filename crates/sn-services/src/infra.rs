use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use serde::Serialize;
use sn_domain::ids::{AdminId, SquadId, TariffId};
use sn_domain::money::Money;
use sn_remnawave::{
    BillingAmount, BillingHistoryQuery, CreateBillingNodeRequest, CreateBillingRecordRequest,
    CreateHostRequest, CreateInfraProviderRequest, CreateInternalSquadRequest, CreateNodeRequest,
    HOST_ALPN, HOST_SECURITY_LAYERS, Host, HostInboundTarget, HostPosition, HostSquadFilter,
    InfraBillingNodes, InternalSquad, Node, NodeDetails, NodeMetadata, NodeProfileSelection,
    NodeState, NodeSystem, SquadAccessibleNodes, UpdateBillingNodesRequest, UpdateHostRequest,
    UpdateInfraProviderRequest, UpdateInternalSquadRequest, UpdateNodeRequest,
};
use sn_store::Db;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeAction {
    Restart,
    Enable,
    Disable,
    ResetTraffic,
    Delete,
}

impl NodeAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "restart" => Some(NodeAction::Restart),
            "enable" => Some(NodeAction::Enable),
            "disable" => Some(NodeAction::Disable),
            "reset-traffic" => Some(NodeAction::ResetTraffic),
            "delete" => Some(NodeAction::Delete),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            NodeAction::Restart => "restart",
            NodeAction::Enable => "enable",
            NodeAction::Disable => "disable",
            NodeAction::ResetTraffic => "reset_traffic",
            NodeAction::Delete => "delete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeView {
    pub node: Node,
    pub state: NodeState,
    pub traffic_alert: bool,
    pub city: Option<String>,
    pub system: Option<NodeSystem>,
    pub inbounds: Vec<InboundBrief>,
    pub profile_uuid: Option<Uuid>,
    pub squads: Vec<String>,
    pub billing: Option<NodeBilling>,
    pub down_since: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InboundBrief {
    pub uuid: Uuid,
    pub tag: String,
    pub kind: String,
    pub security: Option<String>,
    pub network: Option<String>,
    pub port: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeBilling {
    pub uuid: Uuid,
    pub provider: String,
    pub login_url: Option<String>,
    pub next_billing_at: chrono::DateTime<chrono::Utc>,
    pub price: Option<Money>,
}

impl From<Node> for NodeView {
    fn from(node: Node) -> Self {
        NodeView {
            state: node.state(),
            traffic_alert: node.traffic_alert(),
            node,
            city: None,
            system: None,
            inbounds: Vec::new(),
            profile_uuid: None,
            squads: Vec::new(),
            billing: None,
            down_since: None,
        }
    }
}

fn inbound_brief(inbound: &sn_remnawave::Inbound) -> InboundBrief {
    InboundBrief {
        uuid: inbound.uuid,
        tag: inbound.tag.clone(),
        kind: inbound.kind.clone(),
        security: inbound.security.clone(),
        network: inbound.network.clone(),
        port: inbound.port,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileView {
    pub uuid: Uuid,
    pub name: String,
    pub inbounds: Vec<InboundBrief>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderView {
    pub uuid: Uuid,
    pub name: String,
    pub login_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct NewNodeInput {
    pub name: String,
    pub address: String,
    #[serde(default)]
    pub port: Option<i64>,
    #[serde(default)]
    pub country_code: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
    pub profile_uuid: Uuid,
    pub inbounds: Vec<Uuid>,
    #[serde(default)]
    pub provider_uuid: Option<Uuid>,
    #[serde(default)]
    pub next_billing_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CreatedNode {
    pub node: Node,
    pub port: i64,
    pub secret_key: String,
}

pub const NODE_PORT: i64 = 2222;
pub const PRICES_KEY: &str = "infra.prices";
pub const HOST_REMARK_MAX: usize = 100;
pub const HOST_ADDRESS_MAX: usize = 253;
pub const HOST_HEADER_MAX: usize = 512;
pub const HOST_PATH_MAX: usize = 512;
const SECURITY_DEFAULT: &str = "DEFAULT";

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct HostInput {
    #[serde(default)]
    pub inbound: Option<Uuid>,
    pub remark: String,
    pub address: String,
    pub port: i64,
    #[serde(default)]
    pub sni: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub alpn: Option<String>,
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub security_layer: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostAction {
    Enable,
    Disable,
}

impl HostAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "enable" => Some(HostAction::Enable),
            "disable" => Some(HostAction::Disable),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostNodeView {
    pub uuid: Uuid,
    pub name: String,
    pub country_code: String,
    pub address: String,
    pub state: NodeState,
    pub inbounds: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostInboundView {
    #[serde(flatten)]
    pub inbound: InboundBrief,
    pub profile_uuid: Option<Uuid>,
    pub profile_name: String,
    pub nodes: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostSquadRef {
    pub uuid: Uuid,
    pub name: String,
    pub shown: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostRow {
    pub host: Host,
    pub nodes: Vec<Uuid>,
    pub squads: Vec<HostSquadRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostsView {
    pub hosts: Vec<HostRow>,
    pub nodes: Vec<HostNodeView>,
    pub inbounds: Vec<HostInboundView>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct HostOrderInput {
    pub uuids: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct HostChange {
    pub uuid: Uuid,
    #[serde(default)]
    pub remark: Option<String>,
    #[serde(default)]
    pub shown: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct SquadInput {
    pub name: String,
    #[serde(default)]
    pub inbounds: Vec<Uuid>,
    #[serde(default)]
    pub hosts: Vec<HostChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct NodeEditInput {
    pub name: String,
    pub address: String,
    #[serde(default)]
    pub country_code: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
}

fn squad_name(raw: &str) -> ServiceResult<&str> {
    let name = raw.trim();
    let allowed = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-'));
    if !(2..=30).contains(&name.chars().count()) || !allowed {
        return Err(ServiceError::rejected(
            "squad.name",
            "название сквада: от 2 до 30 символов, латиница, цифры, пробел, дефис или подчёркивание",
        ));
    }
    Ok(name)
}

fn node_name(raw: &str) -> ServiceResult<&str> {
    let name = raw.trim();
    if !(3..=30).contains(&name.chars().count()) {
        return Err(ServiceError::rejected(
            "node.name",
            "название ноды от 3 до 30 символов",
        ));
    }
    Ok(name)
}

fn node_address(raw: &str) -> ServiceResult<&str> {
    let address = raw.trim();
    if !(2..=253).contains(&address.len()) || address.contains(char::is_whitespace) {
        return Err(ServiceError::rejected(
            "node.address",
            "укажите IP или домен сервера без пробелов",
        ));
    }
    Ok(address)
}

fn country_code(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|c| c.len() == 2 && c.chars().all(|ch| ch.is_ascii_alphabetic()))
        .map(str::to_uppercase)
}

fn valid_remark(raw: &str) -> ServiceResult<&str> {
    let remark = raw.trim();
    if remark.is_empty() || remark.chars().count() > HOST_REMARK_MAX {
        return Err(ServiceError::rejected(
            "host.remark",
            format!("название сервера от 1 до {HOST_REMARK_MAX} символов"),
        ));
    }
    Ok(remark)
}

fn host_remark(change: &HostChange) -> ServiceResult<Option<&str>> {
    change.remark.as_deref().map(valid_remark).transpose()
}

fn host_text(
    raw: Option<&str>,
    code: &'static str,
    label: &str,
    max: usize,
) -> ServiceResult<Option<String>> {
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ServiceError::rejected(
            code,
            format!("{label}: без пробелов, до {max} символов"),
        ));
    }
    Ok(Some(value.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HostFields {
    remark: String,
    address: String,
    port: i64,
    sni: Option<String>,
    host: Option<String>,
    path: Option<String>,
    alpn: Option<String>,
    fingerprint: Option<String>,
    security_layer: String,
}

fn host_fields(input: &HostInput) -> ServiceResult<HostFields> {
    let remark = valid_remark(&input.remark)?.to_string();
    let address = input.address.trim();
    if address.is_empty()
        || address.chars().count() > HOST_ADDRESS_MAX
        || address.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(ServiceError::rejected(
            "host.address",
            "укажите IP или домен без пробелов",
        ));
    }
    if !(1..=65535).contains(&input.port) {
        return Err(ServiceError::rejected("host.port", "порт от 1 до 65535"));
    }
    let alpn = input
        .alpn
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.replace(' ', ""));
    if alpn.as_deref().is_some_and(|v| !HOST_ALPN.contains(&v)) {
        return Err(ServiceError::rejected(
            "host.alpn",
            format!("ALPN: один из вариантов {}", HOST_ALPN.join("; ")),
        ));
    }
    let fingerprint = input
        .fingerprint
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    if fingerprint.is_some_and(|v| {
        v.len() > 32
            || !v
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    }) {
        return Err(ServiceError::rejected(
            "host.fingerprint",
            "отпечаток: латиница, цифры, - и _, до 32 символов",
        ));
    }
    let security_layer = input
        .security_layer
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map_or_else(|| SECURITY_DEFAULT.to_string(), str::to_uppercase);
    if !HOST_SECURITY_LAYERS.contains(&security_layer.as_str()) {
        return Err(ServiceError::rejected(
            "host.security",
            "безопасность: как у входа, TLS или без TLS",
        ));
    }
    Ok(HostFields {
        remark,
        address: address.to_string(),
        port: input.port,
        sni: host_text(input.sni.as_deref(), "host.sni", "SNI", HOST_ADDRESS_MAX)?,
        host: host_text(input.host.as_deref(), "host.host", "Host", HOST_HEADER_MAX)?,
        path: host_text(input.path.as_deref(), "host.path", "Path", HOST_PATH_MAX)?,
        alpn,
        fingerprint: fingerprint.map(str::to_string),
        security_layer,
    })
}

fn current_text(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn changed_text(current: Option<&str>, next: &Option<String>) -> Option<Option<String>> {
    (current_text(current) != next.as_deref()).then(|| next.clone())
}

fn host_patch(host: &Host, fields: &HostFields) -> UpdateHostRequest {
    let mut request = UpdateHostRequest::new(host.uuid);
    if host.remark != fields.remark {
        request.remark = Some(fields.remark.clone());
    }
    if host.address != fields.address {
        request.address = Some(fields.address.clone());
    }
    if host.port != fields.port {
        request.port = Some(fields.port);
    }
    request.sni = changed_text(host.sni.as_deref(), &fields.sni);
    request.host = changed_text(host.host.as_deref(), &fields.host);
    request.path = changed_text(host.path.as_deref(), &fields.path);
    request.alpn = changed_text(host.alpn.as_deref(), &fields.alpn);
    request.fingerprint = changed_text(host.fingerprint.as_deref(), &fields.fingerprint);
    let layer = current_text(host.security_layer.as_deref()).unwrap_or(SECURITY_DEFAULT);
    if layer != fields.security_layer {
        request.security_layer = Some(fields.security_layer.clone());
    }
    request
}

fn same_address(a: &str, b: &str) -> bool {
    let a = a.trim();
    !a.is_empty() && a.eq_ignore_ascii_case(b.trim())
}

pub fn host_nodes(host: &Host, nodes: &[HostNodeView]) -> Vec<Uuid> {
    let inbound = host.inbound.config_profile_inbound_uuid;
    let serving: Vec<&HostNodeView> = nodes
        .iter()
        .filter(|n| inbound.is_some_and(|i| n.inbounds.contains(&i)))
        .collect();
    let uuids = |list: Vec<&HostNodeView>| list.into_iter().map(|n| n.uuid).collect::<Vec<_>>();
    let bound: Vec<&HostNodeView> = nodes
        .iter()
        .filter(|n| host.nodes.contains(&n.uuid))
        .collect();
    if !bound.is_empty() {
        let both: Vec<&HostNodeView> = bound
            .iter()
            .copied()
            .filter(|n| serving.iter().any(|s| s.uuid == n.uuid))
            .collect();
        return uuids(if both.is_empty() { bound } else { both });
    }
    if serving.len() == 1 {
        return uuids(serving);
    }
    let pool: Vec<&HostNodeView> = if serving.is_empty() {
        nodes.iter().collect()
    } else {
        serving.clone()
    };
    let by_address: Vec<&HostNodeView> = pool
        .into_iter()
        .filter(|n| same_address(&n.address, &host.address))
        .collect();
    if by_address.is_empty() {
        uuids(serving)
    } else {
        uuids(by_address)
    }
}

fn host_update(host: &Host, squad: Uuid, change: &HostChange) -> ServiceResult<UpdateHostRequest> {
    let mut request = UpdateHostRequest::new(host.uuid);
    if let Some(remark) = host_remark(change)?
        && remark != host.remark
    {
        request.remark = Some(remark.to_string());
    }
    match change.shown {
        Some(true) if !host.shown_to(squad) => {
            if host.is_disabled || host.is_hidden {
                request.is_disabled = host.is_disabled.then_some(false);
                request.is_hidden = host.is_hidden.then_some(false);
                request.internal_squads = Some(HostSquadFilter::allow_only(vec![squad]));
            } else if let Some(filter) = &host.internal_squads {
                request.internal_squads = Some(if filter.is_allow_only() {
                    let mut squads = filter.squads.clone();
                    squads.push(squad);
                    HostSquadFilter::allow_only(squads)
                } else {
                    HostSquadFilter::exclude(
                        filter
                            .squads
                            .iter()
                            .copied()
                            .filter(|s| *s != squad)
                            .collect(),
                    )
                });
            }
        }
        Some(false) if host.shown_to(squad) => {
            request.internal_squads = Some(match &host.internal_squads {
                Some(filter) if filter.is_allow_only() => HostSquadFilter::allow_only(
                    filter
                        .squads
                        .iter()
                        .copied()
                        .filter(|s| *s != squad)
                        .collect(),
                ),
                Some(filter) => {
                    let mut squads = filter.squads.clone();
                    squads.push(squad);
                    HostSquadFilter::exclude(squads)
                }
                None => HostSquadFilter::exclude(vec![squad]),
            });
        }
        _ => {}
    }
    Ok(request)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ProviderInput {
    pub name: String,
    #[serde(default)]
    pub login_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BillingRow {
    pub uuid: Uuid,
    pub node_uuid: Option<Uuid>,
    pub node_name: String,
    pub country_code: String,
    pub city: Option<String>,
    pub provider: ProviderView,
    pub next_billing_at: chrono::DateTime<chrono::Utc>,
    pub price: Option<Money>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderSummary {
    pub provider: ProviderView,
    pub nodes: usize,
    pub monthly: Money,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaymentRecord {
    pub provider: String,
    pub nodes: Vec<String>,
    pub amount: Money,
    pub billed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaymentHistory {
    pub records: Vec<PaymentRecord>,
    pub total: i64,
}

pub const PAID_ACTION: &str = "infra.billing_paid";
pub const HISTORY_PAGE_MAX: u32 = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BillingView {
    pub rows: Vec<BillingRow>,
    pub providers: Vec<ProviderSummary>,
    pub history: Vec<PaymentRecord>,
    pub history_total: i64,
    pub month_total: Money,
    pub month_paid: Money,
    pub spent_total: Money,
    pub revenue_30d: Option<Money>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodesView {
    pub total: usize,
    pub online: usize,
    pub online_users: i64,
    pub attention: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TariffRef {
    pub id: TariffId,
    pub name: String,
    pub on_sale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SquadView {
    pub squad: InternalSquad,
    pub tariffs: Vec<TariffRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeActionResult {
    pub action: NodeAction,
    pub node: Option<Node>,
}

pub struct InfraOps<'a> {
    services: &'a Services,
}

impl<'a> InfraOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        InfraOps { services }
    }

    pub async fn nodes(&self) -> ServiceResult<(NodesView, Vec<NodeView>)> {
        let remna = self.services.remna()?;
        let details = match remna.nodes_detailed().await {
            Ok(details) => details,
            Err(err) => {
                tracing::warn!(error = %err, "подробности нод не получены, беру краткий список");
                remna
                    .nodes()
                    .await?
                    .into_iter()
                    .map(sn_remnawave::NodeDetails::from)
                    .collect()
            }
        };
        let mut squads_of: HashMap<Uuid, Vec<String>> = HashMap::new();
        if let Ok(squads) = remna.internal_squads().await {
            for squad in squads.internal_squads {
                if let Ok(nodes) = remna.squad_accessible_nodes(squad.uuid).await {
                    for node in nodes.accessible_nodes {
                        squads_of
                            .entry(node.uuid)
                            .or_default()
                            .push(squad.name.clone());
                    }
                }
            }
        }
        let prices = self.prices().await?;
        let currency = self.services.settings().currency;
        let billing: HashMap<Uuid, NodeBilling> = match remna.infra_billing_nodes().await {
            Ok(list) => list
                .billing_nodes
                .into_iter()
                .filter_map(|b| {
                    let node = b.node_uuid.or(b.node.as_ref().map(|n| n.uuid))?;
                    Some((
                        node,
                        NodeBilling {
                            uuid: b.uuid,
                            provider: b.provider.name,
                            login_url: b.provider.login_url,
                            next_billing_at: b.next_billing_at,
                            price: prices.get(&node).map(|minor| Money::new(*minor, currency)),
                        },
                    ))
                })
                .collect(),
            Err(_) => HashMap::new(),
        };
        let down = self
            .services
            .node_watch()
            .down_since()
            .await
            .unwrap_or_default();
        let mut nodes = Vec::with_capacity(details.len());
        for detail in details {
            let uuid = detail.node.uuid;
            let city = match remna.node_metadata(uuid).await {
                Ok(meta) => meta.city().map(str::to_string),
                Err(_) => None,
            };
            let inbounds = detail
                .config_profile
                .as_ref()
                .map(|p| p.active_inbounds.iter().map(inbound_brief).collect())
                .unwrap_or_default();
            let profile_uuid = detail
                .config_profile
                .as_ref()
                .and_then(|p| p.active_config_profile_uuid);
            let mut view = NodeView::from(detail.node);
            view.city = city;
            view.system = detail.system;
            view.inbounds = inbounds;
            view.profile_uuid = profile_uuid;
            view.squads = squads_of.remove(&uuid).unwrap_or_default();
            view.billing = billing.get(&uuid).cloned();
            view.down_since = down
                .get(&uuid)
                .copied()
                .filter(|_| matches!(view.state, NodeState::Down | NodeState::Connecting));
            nodes.push(view);
        }
        let now = self.services.now();
        let summary = NodesView {
            total: nodes.len(),
            online: nodes
                .iter()
                .filter(|n| n.state == NodeState::Online)
                .count(),
            online_users: nodes.iter().map(|n| n.node.users_online).sum(),
            attention: nodes
                .iter()
                .filter(|n| {
                    matches!(n.state, NodeState::Down | NodeState::Connecting)
                        || n.traffic_alert
                        || n.billing.as_ref().is_some_and(|b| {
                            let left = b.next_billing_at - now;
                            left <= chrono::Duration::days(7)
                        })
                })
                .count(),
        };
        Ok((summary, nodes))
    }

    pub async fn profiles(&self) -> ServiceResult<Vec<ProfileView>> {
        let profiles = self.services.remna()?.config_profiles().await?;
        let mut out: Vec<(i64, ProfileView)> = profiles
            .config_profiles
            .into_iter()
            .map(|p| {
                (
                    p.view_position,
                    ProfileView {
                        uuid: p.uuid,
                        name: p.name,
                        inbounds: p.inbounds.iter().map(inbound_brief).collect(),
                    },
                )
            })
            .collect();
        out.sort_by_key(|(position, _)| *position);
        Ok(out.into_iter().map(|(_, p)| p).collect())
    }

    pub async fn providers(&self) -> ServiceResult<Vec<ProviderView>> {
        Ok(self
            .services
            .remna()?
            .infra_providers()
            .await?
            .providers
            .into_iter()
            .map(|p| ProviderView {
                uuid: p.uuid,
                name: p.name,
                login_url: p.login_url,
            })
            .collect())
    }

    pub async fn create_node(
        &self,
        input: &NewNodeInput,
        admin: AdminId,
    ) -> ServiceResult<CreatedNode> {
        let name = node_name(&input.name)?;
        let address = node_address(&input.address)?;
        if input.inbounds.is_empty() {
            return Err(ServiceError::rejected(
                "node.inbounds",
                "выберите хотя бы один вход",
            ));
        }
        let port = input.port.unwrap_or(NODE_PORT);
        if !(1..=65535).contains(&port) {
            return Err(ServiceError::rejected("node.port", "порт от 1 до 65535"));
        }
        let remna = self.services.remna()?;
        let mut request = CreateNodeRequest::new(
            name,
            address,
            NodeProfileSelection {
                active_config_profile_uuid: input.profile_uuid,
                active_inbounds: input.inbounds.clone(),
            },
        );
        request.port = Some(port);
        request.country_code = country_code(input.country_code.as_deref());
        request.provider_uuid = input.provider_uuid;
        request.is_traffic_tracking_active = Some(false);
        let created = remna.create_node(&request).await?;
        let uuid = created.node.uuid;
        if let Some(city) = input
            .city
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            let mut meta = NodeMetadata::default();
            meta.set_city(Some(city));
            if let Err(err) = remna.set_node_metadata(uuid, &meta).await {
                tracing::warn!(node = %uuid, error = %err, "город ноды не записан");
            }
        }
        if let (Some(provider), Some(next)) = (input.provider_uuid, input.next_billing_at) {
            let billing = CreateBillingNodeRequest {
                provider_uuid: provider,
                node_uuid: Some(uuid),
                name: None,
                next_billing_at: next,
            };
            if let Err(err) = remna.create_infra_billing_node(&billing).await {
                tracing::warn!(node = %uuid, error = %err, "дата оплаты ноды не записана");
            }
        }
        let secret_key = remna.keygen().await?.secret_key;
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                "node.create",
                "node",
                None,
                serde_json::json!({ "node": uuid, "name": name, "address": address }),
            ))
            .await?;
        Ok(CreatedNode {
            node: created.node,
            port,
            secret_key,
        })
    }

    pub async fn node_action(
        &self,
        uuid: Uuid,
        action: NodeAction,
        admin: AdminId,
    ) -> ServiceResult<NodeActionResult> {
        let remna = self.services.remna()?;
        let node = match action {
            NodeAction::Restart => {
                remna.restart_node(uuid).await?;
                None
            }
            NodeAction::Enable => Some(remna.enable_node(uuid).await?),
            NodeAction::Disable => Some(remna.disable_node(uuid).await?),
            NodeAction::ResetTraffic => {
                remna.reset_node_traffic(uuid).await?;
                None
            }
            NodeAction::Delete => {
                remna.delete_node(uuid).await?;
                None
            }
        };
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                &format!("node.{}", action.as_str()),
                "node",
                None,
                serde_json::json!({ "node": uuid }),
            ))
            .await?;
        Ok(NodeActionResult { action, node })
    }

    pub async fn squads(&self) -> ServiceResult<Vec<SquadView>> {
        let squads = self.services.remna()?.internal_squads().await?;
        let mut conn = self.services.store().conn().await?;
        let tariffs = conn.tariffs().list(true).await?;
        Ok(squads
            .internal_squads
            .into_iter()
            .map(|squad| {
                let id = SquadId(squad.uuid);
                let tariffs = tariffs
                    .iter()
                    .filter(|t| t.squads.contains(&id) || t.external_squad == Some(id))
                    .map(|t| TariffRef {
                        id: t.id,
                        name: t.name.clone(),
                        on_sale: t.on_sale,
                    })
                    .collect();
                SquadView { squad, tariffs }
            })
            .collect())
    }

    pub async fn hosts(&self) -> ServiceResult<HostsView> {
        let remna = self.services.remna()?;
        let mut hosts = remna.hosts().await?;
        hosts.sort_by_key(|h| h.view_position);
        let details = match remna.nodes_detailed().await {
            Ok(details) => details,
            Err(err) => {
                tracing::warn!(error = %err, "подробности нод для хостов не получены, беру краткий список");
                match remna.nodes().await {
                    Ok(list) => list.into_iter().map(NodeDetails::from).collect(),
                    Err(err) => {
                        tracing::warn!(error = %err, "ноды для хостов не получены");
                        Vec::new()
                    }
                }
            }
        };
        let profiles = match remna.config_profiles().await {
            Ok(list) => {
                let mut profiles = list.config_profiles;
                profiles.sort_by_key(|p| p.view_position);
                profiles
            }
            Err(err) => {
                tracing::warn!(error = %err, "профили конфигурации для хостов не получены");
                Vec::new()
            }
        };
        let squads = match remna.internal_squads().await {
            Ok(list) => list.internal_squads,
            Err(err) => {
                tracing::warn!(error = %err, "сквады для хостов не получены");
                Vec::new()
            }
        };
        let nodes: Vec<HostNodeView> = details
            .iter()
            .map(|d| HostNodeView {
                uuid: d.node.uuid,
                name: d.node.name.clone(),
                country_code: d.node.country_code.clone(),
                address: d.node.address.clone(),
                state: d.node.state(),
                inbounds: d
                    .config_profile
                    .as_ref()
                    .map(|p| p.active_inbounds.iter().map(|i| i.uuid).collect())
                    .unwrap_or_default(),
            })
            .collect();
        let serving = |inbound: Uuid| -> Vec<Uuid> {
            nodes
                .iter()
                .filter(|n| n.inbounds.contains(&inbound))
                .map(|n| n.uuid)
                .collect()
        };
        let mut inbounds: Vec<HostInboundView> = Vec::new();
        for profile in &profiles {
            for inbound in &profile.inbounds {
                if inbounds.iter().all(|i| i.inbound.uuid != inbound.uuid) {
                    inbounds.push(HostInboundView {
                        inbound: inbound_brief(inbound),
                        profile_uuid: Some(profile.uuid),
                        profile_name: profile.name.clone(),
                        nodes: serving(inbound.uuid),
                    });
                }
            }
        }
        for detail in &details {
            let Some(config) = &detail.config_profile else {
                continue;
            };
            for inbound in &config.active_inbounds {
                if inbounds.iter().all(|i| i.inbound.uuid != inbound.uuid) {
                    inbounds.push(HostInboundView {
                        inbound: inbound_brief(inbound),
                        profile_uuid: inbound.profile_uuid.or(config.active_config_profile_uuid),
                        profile_name: String::new(),
                        nodes: serving(inbound.uuid),
                    });
                }
            }
        }
        let hosts = hosts
            .into_iter()
            .map(|host| {
                let inbound = host.inbound.config_profile_inbound_uuid;
                let squads = squads
                    .iter()
                    .filter(|s| inbound.is_some_and(|i| s.inbounds.iter().any(|x| x.uuid == i)))
                    .map(|s| HostSquadRef {
                        uuid: s.uuid,
                        name: s.name.clone(),
                        shown: host
                            .internal_squads
                            .as_ref()
                            .is_none_or(|filter| filter.allows(s.uuid)),
                    })
                    .collect();
                HostRow {
                    nodes: host_nodes(&host, &nodes),
                    squads,
                    host,
                }
            })
            .collect();
        Ok(HostsView {
            hosts,
            nodes,
            inbounds,
        })
    }

    pub async fn create_host(&self, input: &HostInput, admin: AdminId) -> ServiceResult<Host> {
        let fields = host_fields(input)?;
        let inbound = input
            .inbound
            .ok_or_else(|| ServiceError::rejected("host.inbound", "выберите вход"))?;
        let remna = self.services.remna()?;
        let profiles = remna.config_profiles().await?;
        let profile = profiles
            .config_profiles
            .iter()
            .find(|p| p.inbounds.iter().any(|i| i.uuid == inbound))
            .ok_or(ServiceError::NotFound("вход"))?;
        let mut request = CreateHostRequest::new(
            HostInboundTarget {
                config_profile_uuid: profile.uuid,
                config_profile_inbound_uuid: inbound,
            },
            fields.remark,
            fields.address,
            fields.port,
        );
        request.sni = fields.sni;
        request.host = fields.host;
        request.path = fields.path;
        request.alpn = fields.alpn;
        request.fingerprint = fields.fingerprint;
        request.security_layer = Some(fields.security_layer);
        let host = remna.create_host(&request).await?;
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                "host.create",
                "host",
                None,
                serde_json::json!({
                    "host": host.uuid,
                    "remark": host.remark,
                    "address": host.address,
                    "port": host.port,
                    "inbound": inbound,
                }),
            ))
            .await?;
        Ok(host)
    }

    pub async fn update_host(
        &self,
        uuid: Uuid,
        input: &HostInput,
        admin: AdminId,
    ) -> ServiceResult<Host> {
        let fields = host_fields(input)?;
        let remna = self.services.remna()?;
        let current = remna
            .hosts()
            .await?
            .into_iter()
            .find(|h| h.uuid == uuid)
            .ok_or(ServiceError::NotFound("хост"))?;
        let request = host_patch(&current, &fields);
        if request.is_empty() {
            return Ok(current);
        }
        let changed: Vec<&str> = [
            ("remark", request.remark.is_some()),
            ("address", request.address.is_some()),
            ("port", request.port.is_some()),
            ("sni", request.sni.is_some()),
            ("host", request.host.is_some()),
            ("path", request.path.is_some()),
            ("alpn", request.alpn.is_some()),
            ("fingerprint", request.fingerprint.is_some()),
            ("security_layer", request.security_layer.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, set)| set.then_some(name))
        .collect();
        let updated = remna.update_host(&request).await?;
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                "host.update",
                "host",
                None,
                serde_json::json!({ "host": uuid, "remark": updated.remark, "changed": changed }),
            ))
            .await?;
        Ok(updated)
    }

    pub async fn host_action(
        &self,
        uuid: Uuid,
        action: HostAction,
        admin: AdminId,
    ) -> ServiceResult<Host> {
        let mut request = UpdateHostRequest::new(uuid);
        request.is_disabled = Some(action == HostAction::Disable);
        let host = self.services.remna()?.update_host(&request).await?;
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                match action {
                    HostAction::Enable => "host.enable",
                    HostAction::Disable => "host.disable",
                },
                "host",
                None,
                serde_json::json!({ "host": uuid, "remark": host.remark }),
            ))
            .await?;
        Ok(host)
    }

    pub async fn delete_host(&self, uuid: Uuid, admin: AdminId) -> ServiceResult<()> {
        self.services.remna()?.delete_host(uuid).await?;
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                "host.delete",
                "host",
                None,
                serde_json::json!({ "host": uuid }),
            ))
            .await?;
        Ok(())
    }

    pub async fn reorder_hosts(&self, order: &[Uuid], admin: AdminId) -> ServiceResult<()> {
        let mut seen = HashSet::with_capacity(order.len());
        if order.iter().any(|uuid| !seen.insert(*uuid)) {
            return Err(ServiceError::rejected(
                "host.order",
                "хост повторяется в списке",
            ));
        }
        let remna = self.services.remna()?;
        let hosts = remna.hosts().await?;
        if hosts.len() != order.len() || hosts.iter().any(|h| !seen.contains(&h.uuid)) {
            return Err(ServiceError::rejected(
                "host.order",
                "список хостов изменился: обновите страницу",
            ));
        }
        let positions: Vec<HostPosition> = order
            .iter()
            .zip(0_i64..)
            .map(|(uuid, view_position)| HostPosition {
                uuid: *uuid,
                view_position,
            })
            .collect();
        remna.reorder_hosts(&positions).await?;
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                "host.reorder",
                "host",
                None,
                serde_json::json!({ "hosts": order }),
            ))
            .await?;
        Ok(())
    }

    pub async fn update_node(
        &self,
        uuid: Uuid,
        input: &NodeEditInput,
        admin: AdminId,
    ) -> ServiceResult<Node> {
        let name = node_name(&input.name)?;
        let address = node_address(&input.address)?;
        let city = input
            .city
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty());
        if city.is_some_and(|c| c.chars().count() > 60) {
            return Err(ServiceError::rejected(
                "node.city",
                "название города до 60 символов",
            ));
        }
        let remna = self.services.remna()?;
        let mut meta = match remna.node_metadata(uuid).await {
            Ok(meta) => meta,
            Err(err) if err.is_not_found() => NodeMetadata::default(),
            Err(err) => return Err(err.into()),
        };
        let mut request = UpdateNodeRequest::new(uuid);
        request.name = Some(name.to_string());
        request.address = Some(address.to_string());
        request.country_code = country_code(input.country_code.as_deref());
        let updated = remna.update_node(&request).await?;
        if meta.city() != city {
            meta.set_city(city);
            remna.set_node_metadata(uuid, &meta).await?;
        }
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                "node.update",
                "node",
                None,
                serde_json::json!({ "node": uuid, "name": name, "address": address, "city": city }),
            ))
            .await?;
        Ok(updated.node)
    }

    pub async fn save_squad(
        &self,
        uuid: Option<Uuid>,
        input: &SquadInput,
        admin: AdminId,
    ) -> ServiceResult<InternalSquad> {
        let name = squad_name(&input.name)?;
        if uuid.is_none() && input.inbounds.is_empty() {
            return Err(ServiceError::rejected(
                "squad.inbounds",
                "нужен хотя бы один вход",
            ));
        }
        for change in &input.hosts {
            host_remark(change)?;
        }
        let remna = self.services.remna()?;
        let hosts = if input.hosts.is_empty() {
            Vec::new()
        } else {
            remna.hosts().await?
        };
        if let Some(missing) = input
            .hosts
            .iter()
            .find(|c| !hosts.iter().any(|h| h.uuid == c.uuid))
        {
            tracing::warn!(host = %missing.uuid, "хост из запроса не найден в Remnawave");
            return Err(ServiceError::NotFound("хост"));
        }
        let squad = match uuid {
            Some(uuid) => {
                remna
                    .update_internal_squad(&UpdateInternalSquadRequest {
                        uuid,
                        name: Some(name.to_string()),
                        inbounds: Some(input.inbounds.clone()),
                    })
                    .await?
            }
            None => {
                remna
                    .create_internal_squad(&CreateInternalSquadRequest {
                        name: name.to_string(),
                        inbounds: input.inbounds.clone(),
                    })
                    .await?
            }
        };
        let mut changed_hosts = Vec::new();
        for change in &input.hosts {
            let Some(host) = hosts.iter().find(|h| h.uuid == change.uuid) else {
                continue;
            };
            let request = host_update(host, squad.uuid, change)?;
            if !request.is_empty() {
                remna.update_host(&request).await?;
                changed_hosts.push(host.uuid);
            }
        }
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                if uuid.is_some() { "squad.update" } else { "squad.create" },
                "squad",
                None,
                serde_json::json!({ "squad": squad.uuid, "name": squad.name, "inbounds": input.inbounds.len(), "hosts": changed_hosts }),
            ))
            .await?;
        Ok(squad)
    }

    async fn prices(&self) -> ServiceResult<HashMap<Uuid, i64>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .settings()
            .get_as::<HashMap<Uuid, i64>>(PRICES_KEY)
            .await?
            .unwrap_or_default())
    }

    pub async fn set_price(
        &self,
        node: Uuid,
        minor: Option<i64>,
        admin: AdminId,
    ) -> ServiceResult<()> {
        if minor.is_some_and(|m| !(0..=100_000_000).contains(&m)) {
            return Err(ServiceError::rejected(
                "infra.price",
                "цена вне допустимых пределов",
            ));
        }
        let mut prices = self.prices().await?;
        match minor {
            Some(value) => {
                prices.insert(node, value);
            }
            None => {
                prices.remove(&node);
            }
        }
        let mut conn = self.services.store().conn().await?;
        conn.settings()
            .set_as(PRICES_KEY, &prices, Some(admin))
            .await?;
        Ok(())
    }

    pub async fn billing_view(&self, include_revenue: bool) -> ServiceResult<BillingView> {
        let currency = self.services.settings().currency;
        let remna = self.services.remna()?;
        let billing = remna.infra_billing_nodes().await?;
        let prices = self.prices().await?;
        let mut rows: Vec<BillingRow> = billing
            .billing_nodes
            .iter()
            .map(|b| {
                let node_uuid = b.node_uuid.or(b.node.as_ref().map(|n| n.uuid));
                BillingRow {
                    uuid: b.uuid,
                    node_uuid,
                    node_name: b
                        .node
                        .as_ref()
                        .map(|n| n.name.clone())
                        .or_else(|| b.name.clone())
                        .unwrap_or_else(|| "нода".into()),
                    country_code: b
                        .node
                        .as_ref()
                        .map(|n| n.country_code.clone())
                        .unwrap_or_default(),
                    city: None,
                    provider: ProviderView {
                        uuid: b.provider.uuid,
                        name: b.provider.name.clone(),
                        login_url: b.provider.login_url.clone(),
                    },
                    next_billing_at: b.next_billing_at,
                    price: node_uuid
                        .and_then(|u| prices.get(&u))
                        .map(|minor| Money::new(*minor, currency)),
                }
            })
            .collect();
        rows.sort_by_key(|r| r.next_billing_at);
        for row in &mut rows {
            if let Some(node) = row.node_uuid {
                row.city = match remna.node_metadata(node).await {
                    Ok(meta) => meta.city().map(str::to_string),
                    Err(_) => None,
                };
            }
        }
        let mut providers: Vec<ProviderSummary> = Vec::new();
        for row in &rows {
            match providers
                .iter_mut()
                .find(|p| p.provider.uuid == row.provider.uuid)
            {
                Some(summary) => {
                    summary.nodes += 1;
                    summary.monthly = Money::new(
                        summary.monthly.minor + row.price.map_or(0, |p| p.minor),
                        currency,
                    );
                }
                None => providers.push(ProviderSummary {
                    provider: row.provider.clone(),
                    nodes: 1,
                    monthly: Money::new(row.price.map_or(0, |p| p.minor), currency),
                }),
            }
        }
        if let Ok(all) = remna.infra_providers().await {
            for provider in all.providers {
                if !providers.iter().any(|p| p.provider.uuid == provider.uuid) {
                    providers.push(ProviderSummary {
                        provider: ProviderView {
                            uuid: provider.uuid,
                            name: provider.name,
                            login_url: provider.login_url,
                        },
                        nodes: 0,
                        monthly: Money::new(0, currency),
                    });
                }
            }
        }
        providers.sort_by(|a, b| {
            b.monthly
                .minor
                .cmp(&a.monthly.minor)
                .then_with(|| b.nodes.cmp(&a.nodes))
                .then_with(|| a.provider.name.cmp(&b.provider.name))
        });
        let (history, history_total) = match self.payment_history(0, 5).await {
            Ok(h) => (h.records, h.total),
            Err(err) => {
                tracing::warn!(error = %err, "история оплат серверов не получена");
                (Vec::new(), 0)
            }
        };
        let month_total = Money::new(
            rows.iter().map(|r| r.price.map_or(0, |p| p.minor)).sum(),
            currency,
        );
        let to_minor = |value: f64| (value * 100.0).round() as i64;
        let revenue_30d = if include_revenue {
            let mut conn = self.services.store().conn().await?;
            let stats = conn
                .subscriptions()
                .dashboard(self.services.now(), currency)
                .await
                .ok();
            stats.map(|s| Money::new(s.revenue_30d_minor, currency))
        } else {
            None
        };
        Ok(BillingView {
            rows,
            providers,
            history,
            history_total,
            month_total,
            month_paid: Money::new(to_minor(billing.stats.current_month_payments), currency),
            spent_total: Money::new(to_minor(billing.stats.total_spent), currency),
            revenue_30d,
        })
    }

    pub async fn payment_history(&self, start: u32, size: u32) -> ServiceResult<PaymentHistory> {
        let currency = self.services.settings().currency;
        let history = self
            .services
            .remna()?
            .infra_billing_history(&BillingHistoryQuery {
                start: Some(start),
                size: Some(size.clamp(1, HISTORY_PAGE_MAX)),
            })
            .await?;
        let marks = if history.records.is_empty() {
            Vec::new()
        } else {
            let mut conn = self.services.store().conn().await?;
            conn.audit().recent_by_action(PAID_ACTION, 1_000).await?
        };
        let records = history
            .records
            .into_iter()
            .map(|r| {
                let nodes = marks
                    .iter()
                    .find(|m| {
                        let d = &m.details;
                        d.get("provider").and_then(|v| v.as_str())
                            == Some(r.provider_uuid.to_string().as_str())
                            && d.get("amount_minor").and_then(serde_json::Value::as_i64)
                                == Some(r.amount.minor())
                            && d.get("billed_at")
                                .and_then(|v| v.as_str())
                                .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                                .is_some_and(|at| {
                                    (at.with_timezone(&chrono::Utc) - r.billed_at).abs()
                                        <= chrono::Duration::seconds(2)
                                })
                    })
                    .and_then(|m| m.details.get("node_name").and_then(|v| v.as_str()))
                    .map(|name| vec![name.to_string()])
                    .unwrap_or_default();
                PaymentRecord {
                    provider: r
                        .provider
                        .as_ref()
                        .map(|p| p.name.clone())
                        .unwrap_or_default(),
                    nodes,
                    amount: Money::new(r.amount.minor(), currency),
                    billed_at: r.billed_at,
                }
            })
            .collect();
        Ok(PaymentHistory {
            records,
            total: history.total,
        })
    }

    pub async fn mark_paid(
        &self,
        billing_node: Uuid,
        amount_minor: Option<i64>,
        admin: AdminId,
    ) -> ServiceResult<()> {
        let remna = self.services.remna()?;
        let billing = remna.infra_billing_nodes().await?;
        let row = billing
            .billing_nodes
            .iter()
            .find(|b| b.uuid == billing_node)
            .ok_or(ServiceError::NotFound("оплата ноды"))?;
        let node_uuid = row.node_uuid.or(row.node.as_ref().map(|n| n.uuid));
        let price = match (amount_minor, node_uuid) {
            (Some(value), _) => value,
            (None, Some(node)) => self.prices().await?.get(&node).copied().unwrap_or(0),
            (None, None) => 0,
        };
        if price <= 0 {
            return Err(ServiceError::rejected(
                "infra.price",
                "укажите сумму оплаты: цена сервера не задана",
            ));
        }
        let now = self.services.now();
        remna
            .create_infra_billing_record(&CreateBillingRecordRequest {
                provider_uuid: row.provider.uuid,
                amount: BillingAmount::from_minor(price),
                billed_at: now,
            })
            .await?;
        let node_name = row
            .node
            .as_ref()
            .map(|n| n.name.clone())
            .or_else(|| row.name.clone());
        let next = row
            .next_billing_at
            .checked_add_months(chrono::Months::new(1))
            .unwrap_or(row.next_billing_at + chrono::Duration::days(30));
        remna
            .update_infra_billing_nodes(&UpdateBillingNodesRequest {
                uuids: vec![row.uuid],
                next_billing_at: next,
            })
            .await?;
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                PAID_ACTION,
                "node",
                None,
                serde_json::json!({
                    "billing_node": billing_node,
                    "provider": row.provider.uuid,
                    "node_name": node_name,
                    "amount_minor": price,
                    "billed_at": now,
                    "next_billing_at": next,
                }),
            ))
            .await?;
        Ok(())
    }

    pub async fn save_provider(
        &self,
        uuid: Option<Uuid>,
        input: &ProviderInput,
        admin: AdminId,
    ) -> ServiceResult<ProviderView> {
        let name = input.name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            return Err(ServiceError::rejected(
                "provider.name",
                "название провайдера от 1 до 60 символов",
            ));
        }
        let login_url = input
            .login_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string);
        if login_url
            .as_deref()
            .is_some_and(|u| !(u.starts_with("https://") || u.starts_with("http://")))
        {
            return Err(ServiceError::rejected(
                "provider.url",
                "ссылка на кабинет должна начинаться с https://",
            ));
        }
        let remna = self.services.remna()?;
        let provider = match uuid {
            Some(uuid) => {
                remna
                    .update_infra_provider(&UpdateInfraProviderRequest {
                        uuid,
                        name: Some(name.to_string()),
                        favicon_link: None,
                        login_url: Some(login_url.clone()),
                    })
                    .await?
            }
            None => {
                remna
                    .create_infra_provider(&CreateInfraProviderRequest {
                        name: name.to_string(),
                        favicon_link: None,
                        login_url: login_url.clone(),
                    })
                    .await?
            }
        };
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                if uuid.is_some() {
                    "provider.update"
                } else {
                    "provider.create"
                },
                "provider",
                None,
                serde_json::json!({ "provider": provider.uuid, "name": provider.name }),
            ))
            .await?;
        Ok(ProviderView {
            uuid: provider.uuid,
            name: provider.name,
            login_url: provider.login_url,
        })
    }

    pub async fn squad_nodes(&self, uuid: Uuid) -> ServiceResult<SquadAccessibleNodes> {
        Ok(self.services.remna()?.squad_accessible_nodes(uuid).await?)
    }

    pub async fn billing(&self) -> ServiceResult<InfraBillingNodes> {
        Ok(self.services.remna()?.infra_billing_nodes().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvFallbacks, FixedClock, ServiceDeps};
    use chrono::TimeZone;
    use sn_modules::ModuleRegistry;
    use sn_payments::ProviderRegistry;
    use sn_remnawave::fake::{FakeRemnawave, sample_node};
    use sn_remnawave::{ConfigProfile, HostInbound, Inbound, NodeConfigProfile, SquadInfo};
    use sn_store::testing::test_store;
    use std::sync::Arc;

    const PROFILE: Uuid = Uuid::from_u128(10);
    const REALITY: Uuid = Uuid::from_u128(11);
    const XHTTP: Uuid = Uuid::from_u128(12);
    const DE: Uuid = Uuid::from_u128(21);
    const NL: Uuid = Uuid::from_u128(22);
    const MAIN: Uuid = Uuid::from_u128(31);
    const RESERVE: Uuid = Uuid::from_u128(32);
    const ADMIN: AdminId = AdminId(1);

    fn inbound(uuid: Uuid, tag: &str, network: &str, security: &str, port: i64) -> Inbound {
        Inbound {
            uuid,
            profile_uuid: Some(PROFILE),
            tag: tag.into(),
            kind: "vless".into(),
            network: Some(network.into()),
            security: Some(security.into()),
            port: Some(port),
        }
    }

    fn node(
        uuid: Uuid,
        name: &str,
        country: &str,
        address: &str,
        inbounds: &[Uuid],
    ) -> HostNodeView {
        HostNodeView {
            uuid,
            name: name.into(),
            country_code: country.into(),
            address: address.into(),
            state: NodeState::Online,
            inbounds: inbounds.to_vec(),
        }
    }

    fn host(n: u128, remark: &str, inbound: Uuid, address: &str) -> Host {
        Host {
            uuid: Uuid::from_u128(n),
            view_position: n as i64,
            remark: remark.into(),
            address: address.into(),
            port: 443,
            inbound: HostInbound {
                config_profile_uuid: Some(PROFILE),
                config_profile_inbound_uuid: Some(inbound),
            },
            ..Host::default()
        }
    }

    fn input(remark: &str, address: &str, port: i64) -> HostInput {
        HostInput {
            inbound: Some(REALITY),
            remark: remark.into(),
            address: address.into(),
            port,
            sni: None,
            host: None,
            path: None,
            alpn: None,
            fingerprint: None,
            security_layer: None,
        }
    }

    #[test]
    fn нода_хоста_по_входу_привязке_и_адресу() {
        let nodes = [
            node(DE, "DE-1", "DE", "203.0.113.41", &[REALITY, XHTTP]),
            node(NL, "NL-1", "NL", "198.51.100.21", &[REALITY]),
        ];
        assert_eq!(
            host_nodes(&host(1, "Германия XHTTP", XHTTP, "de.example.com"), &nodes),
            vec![DE]
        );
        assert_eq!(
            host_nodes(&host(2, "Германия", REALITY, " 203.0.113.41 "), &nodes),
            vec![DE]
        );
        let mut bound = host(3, "Нидерланды", REALITY, "nl.example.com");
        bound.nodes = vec![NL];
        assert_eq!(host_nodes(&bound, &nodes), vec![NL]);
        assert_eq!(
            host_nodes(&host(4, "Общий", REALITY, "cdn.example.com"), &nodes),
            vec![DE, NL]
        );
        let stray = Uuid::from_u128(99);
        assert!(host_nodes(&host(5, "Старый", stray, "old.example.com"), &nodes).is_empty());
        assert_eq!(
            host_nodes(&host(6, "Старый", stray, "198.51.100.21"), &nodes),
            vec![NL]
        );
        let mut unbound = host(7, "Без входа", REALITY, "x.example.com");
        unbound.inbound = HostInbound::default();
        assert!(host_nodes(&unbound, &nodes).is_empty());
    }

    #[test]
    fn поля_хоста_проверяются_и_приводятся() {
        let mut raw = input("  Германия  ", " de.example.com ", 443);
        raw.sni = Some("  ".into());
        raw.path = Some(" /xhttp ".into());
        raw.alpn = Some("h2, http/1.1".into());
        raw.fingerprint = Some("chrome".into());
        raw.security_layer = Some("tls".into());
        let fields = host_fields(&raw).unwrap();
        assert_eq!(fields.remark, "Германия");
        assert_eq!(fields.address, "de.example.com");
        assert_eq!(fields.sni, None);
        assert_eq!(fields.path.as_deref(), Some("/xhttp"));
        assert_eq!(fields.alpn.as_deref(), Some("h2,http/1.1"));
        assert_eq!(fields.security_layer, "TLS");
        assert_eq!(
            host_fields(&input("Германия", "de.example.com", 443))
                .unwrap()
                .security_layer,
            "DEFAULT"
        );
        let code = |input: HostInput| host_fields(&input).unwrap_err().code();
        assert_eq!(code(input(" ", "de.example.com", 443)), "host.remark");
        assert_eq!(
            code(input(&"я".repeat(101), "de.example.com", 443)),
            "host.remark"
        );
        assert_eq!(
            code(input("Германия", "de example.com", 443)),
            "host.address"
        );
        assert_eq!(code(input("Германия", "", 443)), "host.address");
        assert_eq!(code(input("Германия", "de.example.com", 0)), "host.port");
        assert_eq!(
            code(input("Германия", "de.example.com", 65536)),
            "host.port"
        );
        let mut bad = input("Германия", "de.example.com", 443);
        bad.sni = Some("www example.com".into());
        assert_eq!(code(bad.clone()), "host.sni");
        bad.sni = None;
        bad.alpn = Some("h4".into());
        assert_eq!(code(bad.clone()), "host.alpn");
        bad.alpn = None;
        bad.fingerprint = Some("chrome; rm".into());
        assert_eq!(code(bad.clone()), "host.fingerprint");
        bad.fingerprint = Some("randomizednoalpn".into());
        assert!(host_fields(&bad).is_ok());
        bad.security_layer = Some("REALITY".into());
        assert_eq!(code(bad), "host.security");
    }

    #[test]
    fn правка_хоста_шлёт_только_изменённые_поля() {
        let mut current = host(1, "Германия", REALITY, "de.example.com");
        current.sni = Some("www.example.com".into());
        current.fingerprint = Some("chrome".into());
        current.path = Some(String::new());
        current.security_layer = None;
        let mut same = input("Германия", "de.example.com", 443);
        same.sni = Some("www.example.com".into());
        same.fingerprint = Some("chrome".into());
        same.security_layer = Some("DEFAULT".into());
        assert!(host_patch(&current, &host_fields(&same).unwrap()).is_empty());
        let mut next = same.clone();
        next.port = 8443;
        next.sni = None;
        next.alpn = Some("h3".into());
        next.security_layer = Some("TLS".into());
        let patch = host_patch(&current, &host_fields(&next).unwrap());
        assert_eq!(
            serde_json::to_value(&patch).unwrap(),
            serde_json::json!({
                "uuid": Uuid::from_u128(1), "port": 8443, "sni": null, "alpn": "h3", "securityLayer": "TLS"
            })
        );
    }

    #[tokio::test]
    async fn хосты_через_сервис_против_подделки() {
        let Some(db) = test_store().await else { return };
        let services = Services::new(ServiceDeps {
            store: db.store.clone(),
            payments: Arc::new(ProviderRegistry::builtin()),
            modules: Arc::new(ModuleRegistry::catalog_only()),
            secrets: None,
            clock: Arc::new(FixedClock::new(
                chrono::Utc
                    .with_ymd_and_hms(2026, 11, 17, 12, 0, 0)
                    .unwrap(),
            )),
            env: EnvFallbacks::default(),
        });
        services.reload().await.unwrap();
        let remna = Arc::new(FakeRemnawave::new());
        services.set_remna(Some(remna.clone()));
        remna.set_config_profiles(vec![ConfigProfile {
            uuid: PROFILE,
            name: "Default-Profile".into(),
            view_position: 0,
            inbounds: vec![
                inbound(REALITY, "VLESS_REALITY", "tcp", "reality", 443),
                inbound(XHTTP, "VLESS_XHTTP", "xhttp", "none", 8443),
            ],
            nodes: Vec::new(),
        }]);
        let detail =
            |uuid: Uuid, name: &str, country: &str, address: &str, active: Vec<Inbound>| {
                let mut node = sample_node(uuid, name, country);
                node.address = address.into();
                let mut detail = NodeDetails::from(node);
                detail.config_profile = Some(NodeConfigProfile {
                    active_config_profile_uuid: Some(PROFILE),
                    active_inbounds: active,
                });
                detail
            };
        remna.set_node_details(vec![
            detail(
                DE,
                "Germany",
                "DE",
                "203.0.113.41",
                vec![
                    inbound(REALITY, "VLESS_REALITY", "tcp", "reality", 443),
                    inbound(XHTTP, "VLESS_XHTTP", "xhttp", "none", 8443),
                ],
            ),
            detail(
                NL,
                "NL-1",
                "NL",
                "198.51.100.21",
                vec![inbound(REALITY, "VLESS_REALITY", "tcp", "reality", 443)],
            ),
        ]);
        remna.set_squads(vec![
            InternalSquad {
                uuid: MAIN,
                name: "Default-Squad".into(),
                view_position: 0,
                info: SquadInfo::default(),
                inbounds: vec![inbound(REALITY, "VLESS_REALITY", "tcp", "reality", 443)],
            },
            InternalSquad {
                uuid: RESERVE,
                name: "Reserve".into(),
                view_position: 1,
                info: SquadInfo::default(),
                inbounds: vec![
                    inbound(REALITY, "VLESS_REALITY", "tcp", "reality", 443),
                    inbound(XHTTP, "VLESS_XHTTP", "xhttp", "none", 8443),
                ],
            },
        ]);
        let mut hidden = host(3, "Нидерланды", REALITY, "198.51.100.21");
        hidden.internal_squads = Some(HostSquadFilter::exclude(vec![MAIN]));
        remna.set_hosts(vec![
            host(5, "Германия XHTTP", XHTTP, "de.example.com"),
            hidden,
            host(1, "Германия", REALITY, "203.0.113.41"),
        ]);
        let infra = services.infra();

        let view = infra.hosts().await.unwrap();
        let remarks: Vec<&str> = view.hosts.iter().map(|r| r.host.remark.as_str()).collect();
        assert_eq!(remarks, ["Германия", "Нидерланды", "Германия XHTTP"]);
        assert_eq!(view.hosts[0].nodes, vec![DE]);
        assert_eq!(view.hosts[1].nodes, vec![NL]);
        assert_eq!(view.hosts[2].nodes, vec![DE]);
        let squads = |row: &HostRow| -> Vec<(String, bool)> {
            row.squads
                .iter()
                .map(|s| (s.name.clone(), s.shown))
                .collect()
        };
        assert_eq!(
            squads(&view.hosts[1]),
            [
                ("Default-Squad".to_string(), false),
                ("Reserve".to_string(), true)
            ]
        );
        assert_eq!(squads(&view.hosts[2]), [("Reserve".to_string(), true)]);
        assert_eq!(view.inbounds.len(), 2);
        assert_eq!(view.inbounds[0].nodes, vec![DE, NL]);
        assert_eq!(view.inbounds[1].profile_name, "Default-Profile");
        assert_eq!(view.nodes[0].inbounds, vec![REALITY, XHTTP]);

        let mut raw = input("Финляндия", "fi.example.com", 443);
        raw.inbound = Some(Uuid::from_u128(77));
        assert!(matches!(
            infra.create_host(&raw, ADMIN).await,
            Err(ServiceError::NotFound(_))
        ));
        raw.inbound = None;
        assert_eq!(
            infra.create_host(&raw, ADMIN).await.unwrap_err().code(),
            "host.inbound"
        );
        raw.inbound = Some(XHTTP);
        raw.path = Some("/xhttp".into());
        raw.security_layer = Some("TLS".into());
        let created = infra.create_host(&raw, ADMIN).await.unwrap();
        assert_eq!(created.inbound.config_profile_uuid, Some(PROFILE));
        assert_eq!(created.inbound.config_profile_inbound_uuid, Some(XHTTP));
        assert_eq!(created.view_position, 6);
        let sent = remna.calls_of("create_host");
        assert_eq!(sent[0]["path"], "/xhttp");
        assert_eq!(sent[0]["securityLayer"], "TLS");
        assert!(sent[0].get("sni").is_none());

        let mut edit = raw.clone();
        edit.port = 8443;
        edit.path = None;
        edit.sni = Some("www.example.com".into());
        let updated = infra.update_host(created.uuid, &edit, ADMIN).await.unwrap();
        assert_eq!(updated.port, 8443);
        assert_eq!(updated.path, None);
        assert_eq!(updated.sni.as_deref(), Some("www.example.com"));
        let patch = remna.calls_of("update_host").pop().unwrap();
        assert_eq!(
            patch,
            serde_json::json!({"uuid": created.uuid, "port": 8443, "sni": "www.example.com", "path": null})
        );
        let before = remna.calls_of("update_host").len();
        infra.update_host(created.uuid, &edit, ADMIN).await.unwrap();
        assert_eq!(remna.calls_of("update_host").len(), before);
        assert!(matches!(
            infra.update_host(Uuid::from_u128(404), &edit, ADMIN).await,
            Err(ServiceError::NotFound(_))
        ));

        let off = infra
            .host_action(created.uuid, HostAction::Disable, ADMIN)
            .await
            .unwrap();
        assert!(off.is_disabled);
        let on = infra
            .host_action(created.uuid, HostAction::Enable, ADMIN)
            .await
            .unwrap();
        assert!(!on.is_disabled);

        let order = [
            created.uuid,
            Uuid::from_u128(1),
            Uuid::from_u128(5),
            Uuid::from_u128(3),
        ];
        infra.reorder_hosts(&order, ADMIN).await.unwrap();
        let view = infra.hosts().await.unwrap();
        let uuids: Vec<Uuid> = view.hosts.iter().map(|r| r.host.uuid).collect();
        assert_eq!(uuids, order);
        assert_eq!(
            infra
                .reorder_hosts(&order[..3], ADMIN)
                .await
                .unwrap_err()
                .code(),
            "host.order"
        );
        assert_eq!(
            infra
                .reorder_hosts(&[order[0], order[0], order[1], order[2]], ADMIN)
                .await
                .unwrap_err()
                .code(),
            "host.order"
        );

        infra.delete_host(created.uuid, ADMIN).await.unwrap();
        assert_eq!(remna.host_list().len(), 3);
        assert!(infra.delete_host(created.uuid, ADMIN).await.is_err());
        let mut conn = services.store().conn().await.unwrap();
        let actions: Vec<String> = conn
            .audit()
            .recent_by_action("host.delete", 10)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.action)
            .collect();
        assert_eq!(actions, ["host.delete"]);
        drop(conn);
        db.cleanup().await;
    }
}
