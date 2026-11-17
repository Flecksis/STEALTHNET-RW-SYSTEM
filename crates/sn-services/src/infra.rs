use crate::Actor;
use crate::error::ServiceResult;
use crate::services::Services;
use serde::Serialize;
use sn_domain::ids::{AdminId, SquadId, TariffId};
use sn_remnawave::{InfraBillingNodes, InternalSquad, Node, NodeState, SquadAccessibleNodes};
use sn_store::Db;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeAction {
    Restart,
    Enable,
    Disable,
}

impl NodeAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "restart" => Some(NodeAction::Restart),
            "enable" => Some(NodeAction::Enable),
            "disable" => Some(NodeAction::Disable),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            NodeAction::Restart => "restart",
            NodeAction::Enable => "enable",
            NodeAction::Disable => "disable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeView {
    pub node: Node,
    pub state: NodeState,
    pub traffic_alert: bool,
}

impl From<Node> for NodeView {
    fn from(node: Node) -> Self {
        NodeView {
            state: node.state(),
            traffic_alert: node.traffic_alert(),
            node,
        }
    }
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
        let nodes: Vec<NodeView> = self
            .services
            .remna()?
            .nodes()
            .await?
            .into_iter()
            .map(NodeView::from)
            .collect();
        let summary = NodesView {
            total: nodes.len(),
            online: nodes
                .iter()
                .filter(|n| n.state == NodeState::Online)
                .count(),
            online_users: nodes.iter().map(|n| n.node.users_online).sum(),
            attention: nodes
                .iter()
                .filter(|n| matches!(n.state, NodeState::Down) || n.traffic_alert)
                .count(),
        };
        Ok((summary, nodes))
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

    pub async fn squad_nodes(&self, uuid: Uuid) -> ServiceResult<SquadAccessibleNodes> {
        Ok(self.services.remna()?.squad_accessible_nodes(uuid).await?)
    }

    pub async fn billing(&self) -> ServiceResult<InfraBillingNodes> {
        Ok(self.services.remna()?.infra_billing_nodes().await?)
    }
}
