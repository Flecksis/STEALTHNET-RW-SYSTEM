mod routes;
pub mod store;
#[cfg(test)]
mod tests;

use crate::catalog::db_now;
use crate::catalog::start_param;
use crate::context::{GrantRefusal, GrantSpec, GrantTarget, ModuleContext, SubscriptionGrant};
use crate::error::ModuleError;
use crate::module::Module;
use async_trait::async_trait;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::ids::{AdminId, ClientId, SquadId};
use sn_domain::module::ModuleId;
use sn_domain::paging::{Page, PageRequest};
use sn_domain::plan::PlanKind;
use sn_domain::settings::keys;
use sn_domain::traffic::{GIB, TrafficPolicy, TrafficReset};
use sn_store::{Db, StoreError};
use uuid::Uuid;

pub use store::ListFilter;

pub const START_PREFIX: &str = "pl_";
pub const MAX_DAYS: u32 = 3_650;
pub const MAX_DEVICES: u32 = 100;
pub const MAX_TRAFFIC_GB: u32 = 1_000_000;
pub const MAX_NAME: usize = 100;
const CODE_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
const GRANT_NAME: &str = "Промо-подписка";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromoLink {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub squads: Vec<SquadId>,
    pub days: u32,
    pub traffic_limit_bytes: Option<u64>,
    pub traffic_reset: TrafficReset,
    pub devices: u32,
    pub max_activations: Option<u32>,
    pub activations: u32,
    pub expires_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub disabled_at: Option<DateTime<Utc>>,
}

impl PromoLink {
    fn traffic(&self) -> TrafficPolicy {
        TrafficPolicy {
            limit_bytes: self.traffic_limit_bytes,
            reset: self.traffic_reset,
        }
    }

    fn grant(&self, client_id: ClientId) -> SubscriptionGrant {
        SubscriptionGrant {
            client_id,
            module: ModuleId::Promolinks,
            reference: format!("promo_link:{}", self.id),
            label: format!(
                "Промо-ссылка «{}»: {}",
                self.name,
                sn_domain::format::days(self.days)
            ),
            spec: GrantSpec::Custom {
                name: GRANT_NAME.into(),
                days: self.days,
                devices: self.devices,
                traffic: self.traffic(),
                squads: self.squads.clone(),
            },
            target: GrantTarget::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    Active,
    Expired,
    Exhausted,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkView {
    #[serde(flatten)]
    pub link: PromoLink,
    pub state: LinkState,
    pub purchased: u32,
    pub bot_link: Option<String>,
}

pub fn bot_link(bot_username: &str, code: &str) -> String {
    format!(
        "https://t.me/{}?start={START_PREFIX}{code}",
        bot_username.trim().trim_start_matches('@')
    )
}

fn valid_code(code: &str) -> bool {
    (4..=32).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn start_code(raw: &str) -> Option<String> {
    start_param::segments(raw)
        .find_map(|segment| segment.strip_prefix(START_PREFIX))
        .filter(|code| valid_code(code))
        .map(str::to_string)
}

pub fn generate_code() -> String {
    Uuid::new_v4()
        .as_bytes()
        .iter()
        .take(8)
        .map(|b| CODE_ALPHABET[(*b as usize) % CODE_ALPHABET.len()] as char)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NewLink {
    pub name: String,
    #[serde(default)]
    pub code: Option<String>,
    pub squads: Vec<SquadId>,
    pub days: u32,
    #[serde(default)]
    pub traffic_gb: Option<u32>,
    #[serde(default)]
    pub traffic_reset: TrafficReset,
    pub devices: u32,
    #[serde(default)]
    pub max_activations: Option<u32>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum LinkRejection {
    NotFound,
    Disabled,
    Expired,
    Exhausted,
    AlreadyUsed,
    Grant(GrantRefusal),
}

impl LinkRejection {
    pub fn code(&self) -> &'static str {
        match self {
            LinkRejection::NotFound => "promolinks.not_found",
            LinkRejection::Disabled => "promolinks.disabled",
            LinkRejection::Expired => "promolinks.expired",
            LinkRejection::Exhausted => "promolinks.exhausted",
            LinkRejection::AlreadyUsed => "promolinks.already_used",
            LinkRejection::Grant(refusal) => refusal.code,
        }
    }

    pub fn message(&self) -> String {
        match self {
            LinkRejection::NotFound => "Такой промо-ссылки нет. Проверьте ссылку.".into(),
            LinkRejection::Disabled => "Эта промо-ссылка больше не работает.".into(),
            LinkRejection::Expired => "Срок действия этой промо-ссылки закончился.".into(),
            LinkRejection::Exhausted => {
                "Подписки по этой ссылке закончились: её уже активировали все, кто успел.".into()
            }
            LinkRejection::AlreadyUsed => {
                "Вы уже получили подписку по этой ссылке. Одна ссылка срабатывает один раз.".into()
            }
            LinkRejection::Grant(refusal) => refusal.message.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("{}", .0.message())]
    Rejected(LinkRejection),
    #[error(transparent)]
    Module(#[from] ModuleError),
}

impl From<StoreError> for LinkError {
    fn from(err: StoreError) -> Self {
        LinkError::Module(ModuleError::Store(err))
    }
}

async fn bot_username(db: &mut impl Db) -> Result<Option<String>, ModuleError> {
    Ok(db
        .settings()
        .get(keys::BOT_USERNAME)
        .await?
        .and_then(|v| v.as_str().map(str::trim).map(str::to_string))
        .filter(|u| !u.is_empty()))
}

fn view(link: PromoLink, purchased: u32, bot: Option<&str>, now: DateTime<Utc>) -> LinkView {
    LinkView {
        state: store::state(&link, now),
        bot_link: bot.map(|b| bot_link(b, &link.code)),
        purchased,
        link,
    }
}

fn invalid(message: impl Into<String>) -> ModuleError {
    ModuleError::Invalid(message.into())
}

pub async fn create(
    ctx: &ModuleContext,
    new: &NewLink,
    admin: Option<AdminId>,
) -> Result<LinkView, ModuleError> {
    let name = new.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(invalid(format!("название — от 1 до {MAX_NAME} знаков")));
    }
    let code = match new.code.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(code) if valid_code(code) => code.to_string(),
        Some(_) => {
            return Err(invalid(
                "код ссылки — от 4 до 32 латинских букв, цифр, «-» или «_»",
            ));
        }
        None => generate_code(),
    };
    let mut squads: Vec<SquadId> = Vec::new();
    for squad in &new.squads {
        if !squads.contains(squad) {
            squads.push(*squad);
        }
    }
    if squads.is_empty() {
        return Err(invalid("выберите хотя бы один сквад"));
    }
    if new.days == 0 || new.days > MAX_DAYS {
        return Err(invalid(format!("срок подписки — от 1 до {MAX_DAYS} дней")));
    }
    if new.devices > MAX_DEVICES {
        return Err(invalid(format!("устройств — не больше {MAX_DEVICES}")));
    }
    let traffic = match new.traffic_gb {
        None | Some(0) => None,
        Some(gb) if gb <= MAX_TRAFFIC_GB => Some(u64::from(gb) * GIB),
        Some(_) => return Err(invalid("трафик — не больше 1 000 000 ГБ")),
    };
    if new.max_activations == Some(0) {
        return Err(invalid("лимит активаций — больше нуля или пусто"));
    }
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    if new.expires_at.is_some_and(|at| at <= now) {
        return Err(invalid("срок действия ссылки уже прошёл"));
    }
    let link = match store::insert(
        &mut tx,
        &store::NewLinkRow {
            code: &code,
            name,
            squads: &squads,
            days: new.days,
            traffic_limit_bytes: traffic,
            traffic_reset: if traffic.is_some() {
                new.traffic_reset
            } else {
                TrafficReset::NoReset
            },
            devices: new.devices,
            max_activations: new.max_activations,
            expires_at: new.expires_at,
            created_by: admin,
        },
    )
    .await
    {
        Ok(link) => link,
        Err(StoreError::Conflict { .. }) => {
            return Err(invalid("ссылка с таким кодом уже есть, придумайте другой"));
        }
        Err(err) => return Err(err.into()),
    };
    tx.audit()
        .record(&sn_store::AuditEntry {
            actor_kind: if admin.is_some() {
                sn_store::ActorKind::Admin
            } else {
                sn_store::ActorKind::System
            },
            actor_id: admin.map(|a| a.0),
            action: "promolinks.created".into(),
            entity_kind: None,
            entity_id: Some(link.id),
            details: serde_json::json!({
                "code": link.code,
                "name": link.name,
                "days": link.days,
                "max_activations": link.max_activations,
            }),
            ip: None,
        })
        .await?;
    let bot = bot_username(&mut tx).await?;
    tx.commit().await?;
    Ok(view(link, 0, bot.as_deref(), now))
}

pub async fn set_enabled(
    ctx: &ModuleContext,
    id: i64,
    enabled: bool,
    admin: Option<AdminId>,
) -> Result<LinkView, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    let link = store::set_enabled(&mut tx, id, enabled)
        .await?
        .ok_or(ModuleError::NotFound("промо-ссылка"))?;
    tx.audit()
        .record(&sn_store::AuditEntry {
            actor_kind: if admin.is_some() {
                sn_store::ActorKind::Admin
            } else {
                sn_store::ActorKind::System
            },
            actor_id: admin.map(|a| a.0),
            action: if enabled {
                "promolinks.enabled".into()
            } else {
                "promolinks.disabled".into()
            },
            entity_kind: None,
            entity_id: Some(link.id),
            details: serde_json::json!({ "code": link.code }),
            ip: None,
        })
        .await?;
    let purchased = store::purchased(&mut tx, link.id).await?;
    let bot = bot_username(&mut tx).await?;
    tx.commit().await?;
    Ok(view(link, purchased, bot.as_deref(), now))
}

pub async fn list(
    ctx: &ModuleContext,
    filter: ListFilter,
    page: PageRequest,
) -> Result<Page<LinkView>, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let now = db_now(&mut conn).await?;
    let bot = bot_username(&mut conn).await?;
    let page = store::list(&mut conn, filter, page, now).await?;
    Ok(Page {
        items: page
            .items
            .into_iter()
            .map(|(link, purchased)| view(link, purchased, bot.as_deref(), now))
            .collect(),
        next: page.next,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Activated {
    pub link_id: i64,
    pub days: u32,
    pub traffic_limit_bytes: Option<u64>,
    pub devices: u32,
    pub plan: PlanKind,
    pub order_id: Uuid,
    pub subscription_id: Option<sn_domain::ids::SubscriptionId>,
}

pub async fn activate(
    ctx: &ModuleContext,
    client_id: ClientId,
    code: &str,
) -> Result<Activated, LinkError> {
    let rejected = |r: LinkRejection| Err(LinkError::Rejected(r));
    if !valid_code(code) {
        return rejected(LinkRejection::NotFound);
    }
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    let Some(link) = store::lock_by_code(&mut tx, code).await? else {
        return rejected(LinkRejection::NotFound);
    };
    if store::activated_by(&mut tx, link.id, client_id).await? {
        return rejected(LinkRejection::AlreadyUsed);
    }
    match store::state(&link, now) {
        LinkState::Active => {}
        LinkState::Disabled => return rejected(LinkRejection::Disabled),
        LinkState::Expired => return rejected(LinkRejection::Expired),
        LinkState::Exhausted => return rejected(LinkRejection::Exhausted),
    }
    let granted = match ctx
        .core
        .grant_subscription(&mut tx, &link.grant(client_id))
        .await?
    {
        Ok(granted) => granted,
        Err(refusal) => {
            tx.rollback().await?;
            return rejected(LinkRejection::Grant(refusal));
        }
    };
    store::record_activation(&mut tx, link.id, client_id, granted.order_id).await?;
    tx.commit().await?;
    Ok(Activated {
        link_id: link.id,
        days: link.days,
        traffic_limit_bytes: link.traffic_limit_bytes,
        devices: link.devices,
        plan: granted.plan,
        order_id: granted.order_public_id,
        subscription_id: granted.subscription_id,
    })
}

pub struct PromolinksModule;

#[async_trait]
impl Module for PromolinksModule {
    fn id(&self) -> ModuleId {
        ModuleId::Promolinks
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::admin(ctx))
    }
}
