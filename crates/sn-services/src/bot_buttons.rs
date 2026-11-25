use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use crate::telegram::{Bot, TgError};
use serde::Serialize;
use serde_json::{Value, json};
use sn_domain::ids::AdminId;
use sn_domain::lang;
use sn_store::Db;

#[cfg(test)]
mod tests;

pub const MENU_KEY: &str = "bot.menu_button";
pub const MENU_ACTION: &str = "settings.bot_menu";
pub const NO_BOT: &str = "bot.not_connected";
pub const TELEGRAM_FAILED: &str = "bot.telegram_failed";

const COMMANDS_KEY: &str = "commands";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MenuButton {
    Default,
    Commands,
    WebApp { text: String, url: String },
}

impl MenuButton {
    pub fn parse(value: &Value) -> Self {
        let text = |at: &str| {
            value
                .pointer(at)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        match value.get("type").and_then(Value::as_str) {
            Some("web_app") => MenuButton::WebApp {
                text: text("/text"),
                url: text("/web_app/url"),
            },
            Some("commands") => MenuButton::Commands,
            _ => MenuButton::Default,
        }
    }

    pub fn json(&self) -> Value {
        match self {
            MenuButton::Default => json!({ "type": "default" }),
            MenuButton::Commands => json!({ "type": "commands" }),
            MenuButton::WebApp { text, url } => json!({
                "type": "web_app",
                "text": text,
                "web_app": { "url": url },
            }),
        }
    }

    pub fn key(&self) -> Option<String> {
        match self {
            MenuButton::Default => None,
            MenuButton::Commands => Some(COMMANDS_KEY.to_string()),
            MenuButton::WebApp { text, url } => Some(format!("{url}\n{text}")),
        }
    }

    pub fn same_place(&self, other: &MenuButton) -> bool {
        match (self, other) {
            (MenuButton::WebApp { url: a, .. }, MenuButton::WebApp { url: b, .. }) => {
                plain(a) == plain(b)
            }
            (MenuButton::WebApp { .. }, _) | (_, MenuButton::WebApp { .. }) => false,
            _ => true,
        }
    }

    fn describe(&self) -> String {
        match self {
            MenuButton::Default | MenuButton::Commands => "меню с командами".into(),
            MenuButton::WebApp { url, .. } => url.clone(),
        }
    }
}

fn plain(url: &str) -> String {
    let url = url.trim();
    let parsed = reqwest::Url::parse(url)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| url.to_string());
    parsed.trim_end_matches('/').to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ButtonsView {
    pub username: Option<String>,
    pub wanted: MenuButton,
    pub menu: MenuButton,
    pub same: bool,
    pub applied: bool,
    pub main_app: bool,
}

pub struct BotButtons<'a> {
    services: &'a Services,
}

impl<'a> BotButtons<'a> {
    pub fn new(services: &'a Services) -> Self {
        BotButtons { services }
    }

    pub fn wanted(&self) -> MenuButton {
        let settings = self.services.settings();
        let Some(url) = settings.miniapp_url() else {
            return MenuButton::Default;
        };
        if !settings.client_access.menu_button {
            return MenuButton::Commands;
        }
        let locale = settings.languages.fallback();
        let text = self
            .services
            .bot_content_in(locale)
            .content
            .button("menu.btn.app", &[])
            .map(|label| label.text)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| {
                lang::sync_scope(locale, || lang::pick("Приложение", "App").to_string())
            });
        MenuButton::WebApp { text, url }
    }

    pub async fn stored(&self) -> ServiceResult<Option<String>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .settings()
            .get(MENU_KEY)
            .await?
            .and_then(|v| v.as_str().map(str::to_string)))
    }

    pub async fn remember(&self, button: &MenuButton) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        conn.settings()
            .set(MENU_KEY, &json!(button.key()), None)
            .await?;
        Ok(())
    }

    pub async fn view(&self) -> ServiceResult<ButtonsView> {
        let bot = self.bot()?;
        self.look(&bot).await
    }

    pub async fn put_menu(&self, admin: AdminId) -> ServiceResult<ButtonsView> {
        let bot = self.bot()?;
        let wanted = self.wanted();
        let before = MenuButton::parse(&bot.menu_button().await.map_err(failed)?);
        bot.set_menu_button(&wanted.json()).await.map_err(failed)?;
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set(MENU_KEY, &json!(wanted.key()), Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                MENU_ACTION,
                "settings",
                None,
                json!({ "from": before.describe(), "to": wanted.describe() }),
            ))
            .await?;
        tx.commit().await?;
        self.look(&bot).await
    }

    async fn look(&self, bot: &Bot) -> ServiceResult<ButtonsView> {
        let (menu, me) = tokio::join!(bot.menu_button(), bot.me());
        let menu = MenuButton::parse(&menu.map_err(failed)?);
        let me = me.map_err(failed)?;
        let wanted = self.wanted();
        let applied = self.stored().await? == wanted.key();
        Ok(ButtonsView {
            username: me
                .get("username")
                .and_then(Value::as_str)
                .map(str::to_string),
            same: wanted.same_place(&menu),
            applied,
            main_app: me
                .get("has_main_web_app")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            wanted,
            menu,
        })
    }

    fn bot(&self) -> ServiceResult<Bot> {
        let token = self
            .services
            .bot_token()
            .ok_or_else(|| ServiceError::rejected(NO_BOT, "Бот не подключён"))?;
        let proxy = self
            .services
            .egress()
            .proxy_for(sn_egress::Route::Telegram, None);
        Bot::quick(
            self.services.payments().telegram_api(),
            &token,
            proxy.as_deref(),
        )
        .map_err(ServiceError::Internal)
    }
}

fn failed(err: TgError) -> ServiceError {
    tracing::debug!(error = ?err, "кнопки бота в Telegram не проверены");
    ServiceError::rejected(TELEGRAM_FAILED, err.readable())
}
