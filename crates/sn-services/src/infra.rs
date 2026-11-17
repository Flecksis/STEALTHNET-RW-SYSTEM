use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use serde::Serialize;
use sn_domain::ids::{AdminId, SquadId, TariffId};
use sn_domain::money::Money;
use sn_remnawave::{
    BillingAmount, BillingHistoryQuery, CreateBillingNodeRequest, CreateBillingRecordRequest,
    CreateInfraProviderRequest, CreateInternalSquadRequest, CreateNodeRequest, Host,
    HostSquadFilter, InfraBillingNodes, InternalSquad, Node, NodeMetadata, NodeProfileSelection,
    NodeState, NodeSystem, SquadAccessibleNodes, UpdateBillingNodesRequest, UpdateHostRequest,
    UpdateInfraProviderRequest, UpdateInternalSquadRequest, UpdateNodeRequest,
};
use sn_store::Db;
use std::collections::HashMap;
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

fn host_remark(change: &HostChange) -> ServiceResult<Option<&str>> {
    let Some(remark) = change.remark.as_deref().map(str::trim) else {
        return Ok(None);
    };
    if remark.is_empty() || remark.chars().count() > HOST_REMARK_MAX {
        return Err(ServiceError::rejected(
            "host.remark",
            format!("название сервера от 1 до {HOST_REMARK_MAX} символов"),
        ));
    }
    Ok(Some(remark))
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

    pub async fn hosts(&self) -> ServiceResult<Vec<Host>> {
        let mut hosts = self.services.remna()?.hosts().await?;
        hosts.sort_by_key(|h| h.view_position);
        Ok(hosts)
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
