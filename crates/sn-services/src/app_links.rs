use crate::Actor;
use crate::auth::{new_token, token_hash};
use crate::error::{ServiceError, ServiceResult};
use crate::limits::rules;
use crate::services::Services;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sn_domain::client::Locale;
use sn_domain::connect::{self, ConnectApps, LinkButton, LinkSpot, URL_SLOT};
use sn_domain::ids::{ClientId, SubscriptionId};
use sn_store::{Db, StoreError};

pub const LINK_MINUTES: i64 = 10;
pub const EXPIRED: &str = "app_link.expired";
pub const TAKEN: &str = "app_link.taken";
pub const NO_APP: &str = "app_link.no_app";
pub const NOT_READY: &str = "app_link.not_ready";
const TOKEN_LEN: usize = 64;
const HOLDER_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppLink {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenedApp {
    pub name: String,
    pub link: String,
    pub subscription_url: Option<String>,
    pub install: Vec<LinkButton>,
    pub expires_in: i64,
    pub locale: Locale,
}

#[derive(sqlx::FromRow)]
struct LinkRow {
    client_id: i64,
    subscription_id: i64,
    spot: sqlx::types::Json<LinkSpot>,
    expires_at: DateTime<Utc>,
}

fn expired() -> ServiceError {
    ServiceError::rejected(
        EXPIRED,
        "Ссылка устарела. Вернитесь в Telegram и нажмите кнопку ещё раз",
    )
}

fn taken() -> ServiceError {
    ServiceError::rejected(
        TAKEN,
        "Эту ссылку уже открыли в другом окне. Вернитесь в Telegram и нажмите кнопку ещё раз",
    )
}

fn no_app() -> ServiceError {
    ServiceError::rejected(
        NO_APP,
        "Эту кнопку убрали. Вернитесь в Telegram и откройте «Подключение» заново",
    )
}

fn hex_of(raw: &str, len: usize) -> bool {
    raw.len() == len
        && raw
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub struct AppLinks<'a> {
    services: &'a Services,
}

impl<'a> AppLinks<'a> {
    pub fn new(services: &'a Services) -> Self {
        AppLinks { services }
    }

    fn apps(&self) -> ConnectApps {
        self.services.connect_apps().current()
    }

    pub async fn issue(
        &self,
        id: SubscriptionId,
        client_id: ClientId,
        spot: &LinkSpot,
    ) -> ServiceResult<AppLink> {
        self.services
            .limits()
            .hit(rules::APP_LINK_CLIENT, &client_id.0.to_string())
            .await?;
        self.apps().launch_link(spot).ok_or_else(no_app)?;
        let (token, hash) = new_token();
        let now = self.services.now();
        let expires_at = now + Duration::minutes(LINK_MINUTES);
        let mut conn = self.services.store().conn().await?;
        if conn.subscriptions().get(id).await?.client_id != client_id {
            return Err(ServiceError::NotFound("подписка"));
        }
        sqlx::query("DELETE FROM app_links WHERE expires_at <= $1")
            .bind(now)
            .execute(conn.conn())
            .await?;
        sqlx::query(
            "INSERT INTO app_links (token_hash, client_id, subscription_id, spot, created_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(hash)
        .bind(client_id.0)
        .bind(id.0)
        .bind(sqlx::types::Json(spot))
        .bind(now)
        .bind(expires_at)
        .execute(conn.conn())
        .await?;
        Ok(AppLink { token, expires_at })
    }

    async fn claim(&self, token: &str, holder: &str) -> ServiceResult<LinkRow> {
        if !hex_of(token, TOKEN_LEN) || !hex_of(holder, HOLDER_LEN) {
            return Err(expired());
        }
        let now = self.services.now();
        let mut conn = self.services.store().conn().await?;
        let row: Option<LinkRow> = sqlx::query_as(
            "UPDATE app_links SET holder_hash = $2, opened_at = COALESCE(opened_at, $3) \
             WHERE token_hash = $1 AND expires_at > $3 AND (holder_hash IS NULL OR holder_hash = $2) \
             RETURNING client_id, subscription_id, spot, expires_at",
        )
        .bind(token_hash(token))
        .bind(token_hash(holder))
        .bind(now)
        .fetch_optional(conn.conn())
        .await?;
        if let Some(row) = row {
            return Ok(row);
        }
        let live: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM app_links WHERE token_hash = $1 AND expires_at > $2)",
        )
        .bind(token_hash(token))
        .bind(now)
        .fetch_one(conn.conn())
        .await?;
        Err(if live { taken() } else { expired() })
    }

    pub async fn open(&self, token: &str, holder: &str) -> ServiceResult<OpenedApp> {
        let row = self.claim(token, holder).await?;
        let client_id = ClientId(row.client_id);
        let client = {
            let mut conn = self.services.store().conn().await?;
            conn.clients().get(client_id).await?
        };
        if client.is_blocked() {
            return Err(expired());
        }
        let apps = self.apps();
        let (app, template) = apps.launch_link(&row.spot).ok_or_else(no_app)?;
        let url = if template.contains(URL_SLOT) {
            Some(self.subscription_url(&row, client_id).await?)
        } else {
            None
        };
        let brand = self.services.settings().brand_name.clone();
        Ok(OpenedApp {
            name: app.name.clone(),
            link: connect::fill_link(template, url.as_deref().unwrap_or_default(), &brand),
            subscription_url: url,
            install: app.install_buttons().to_vec(),
            expires_in: (row.expires_at - self.services.now()).num_seconds().max(0),
            locale: client.locale,
        })
    }

    async fn subscription_url(&self, row: &LinkRow, client_id: ClientId) -> ServiceResult<String> {
        let view = self
            .services
            .subscriptions()
            .view(
                SubscriptionId(row.subscription_id),
                Actor::Client(client_id),
                true,
            )
            .await;
        match view {
            Ok(view) => view.subscription_url.ok_or_else(|| {
                ServiceError::rejected(
                    NOT_READY,
                    "Подписка ещё не готова: ссылка появится, когда она заработает",
                )
            }),
            Err(ServiceError::NotFound(_) | ServiceError::Store(StoreError::NotFound(_))) => {
                Err(expired())
            }
            Err(err) => Err(err),
        }
    }
}
