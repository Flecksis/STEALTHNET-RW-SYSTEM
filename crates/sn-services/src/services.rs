use crate::admin::AdminOps;
use crate::auth::Auth;
use crate::autorenew::AutoRenew;
use crate::bot_runtime::BotRuntime;
use crate::checkout::Checkout;
use crate::clients::ClientOps;
use crate::clock::Clock;
use crate::error::{ServiceError, ServiceResult};
use crate::events::EventDispatcher;
use crate::fulfillment::Fulfillment;
use crate::inbox::PaymentInbox;
use crate::infra::InfraOps;
use crate::limits::RateLimits;
use crate::remna_sync::RemnaSync;
use crate::settings::{self, SettingsOps};
use crate::stars_review::StarsReview;
use crate::subscriptions::SubscriptionOps;
use crate::support::SupportOps;
use crate::tariffs::TariffAdmin;
use chrono::{DateTime, Utc};
use sn_domain::settings::CoreSettings;
use sn_modules::{BotSender, ModuleContext, ModuleRegistry};
use sn_payments::ProviderRegistry;
use sn_remnawave::{GuardedRemnawave, RemnaConfig, RemnawaveApi, RemnawaveClient};
use sn_store::{Db, SecretBox, Store};
use std::sync::{Arc, RwLock};
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemnaConnection {
    pub url: String,
    pub token: String,
    pub cookie: Option<String>,
    pub webhook_secret: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvFallbacks {
    pub public_url: Option<String>,
    pub remna: Option<RemnaConnection>,
    pub bot_token: Option<String>,
}

pub struct ServiceDeps {
    pub store: Store,
    pub payments: Arc<ProviderRegistry>,
    pub modules: Arc<ModuleRegistry>,
    pub secrets: Option<SecretBox>,
    pub clock: Arc<dyn Clock>,
    pub env: EnvFallbacks,
}

struct Runtime {
    settings: Arc<CoreSettings>,
    remna: Option<Arc<dyn RemnawaveApi>>,
    remna_webhook_secret: Option<String>,
    bot_token: Option<String>,
    updates: Option<Arc<crate::updates::UpdateConfig>>,
    bot_content: Arc<crate::bot_content::BotContent>,
    bot_sender: Option<Arc<dyn BotSender>>,
}

struct Inner {
    store: Store,
    payments: Arc<ProviderRegistry>,
    modules: Arc<ModuleRegistry>,
    secrets: Option<SecretBox>,
    clock: Arc<dyn Clock>,
    env: EnvFallbacks,
    instance: Uuid,
    runtime: RwLock<Runtime>,
    bot: BotRuntime,
}

#[derive(Clone)]
pub struct Services {
    inner: Arc<Inner>,
}

impl Services {
    pub fn new(deps: ServiceDeps) -> Self {
        let runtime = Runtime {
            settings: Arc::new(CoreSettings {
                public_url: deps.env.public_url.clone(),
                ..CoreSettings::default()
            }),
            remna: None,
            remna_webhook_secret: deps
                .env
                .remna
                .as_ref()
                .and_then(|r| r.webhook_secret.clone()),
            bot_token: deps.env.bot_token.clone(),
            updates: None,
            bot_content: Arc::default(),
            bot_sender: None,
        };
        Services {
            inner: Arc::new(Inner {
                store: deps.store,
                payments: deps.payments,
                modules: deps.modules,
                secrets: deps.secrets,
                clock: deps.clock,
                env: deps.env,
                instance: Uuid::new_v4(),
                runtime: RwLock::new(runtime),
                bot: BotRuntime::default(),
            }),
        }
    }

    fn read<T>(&self, f: impl FnOnce(&Runtime) -> T) -> T {
        match self.inner.runtime.read() {
            Ok(guard) => f(&guard),
            Err(poisoned) => f(&poisoned.into_inner()),
        }
    }

    fn write(&self, f: impl FnOnce(&mut Runtime)) {
        match self.inner.runtime.write() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => f(&mut poisoned.into_inner()),
        }
    }

    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    pub fn payments(&self) -> &Arc<ProviderRegistry> {
        &self.inner.payments
    }

    pub fn modules(&self) -> &Arc<ModuleRegistry> {
        &self.inner.modules
    }

    pub fn secrets(&self) -> Option<&SecretBox> {
        self.inner.secrets.as_ref()
    }

    pub fn env(&self) -> &EnvFallbacks {
        &self.inner.env
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.inner.clock.now()
    }

    pub fn instance_id(&self) -> Uuid {
        self.inner.instance
    }

    pub async fn notify_settings_changed(&self) -> ServiceResult<()> {
        let mut conn = self.store().conn().await?;
        sqlx::query("SELECT pg_notify($1, $2)")
            .bind(settings::SETTINGS_CHANNEL)
            .bind(self.inner.instance.to_string())
            .execute(conn.conn())
            .await?;
        Ok(())
    }

    pub fn settings(&self) -> Arc<CoreSettings> {
        self.read(|r| r.settings.clone())
    }

    pub fn replace_settings(&self, settings: CoreSettings) {
        self.inner.modules.set_currency(settings.currency);
        self.write(|r| r.settings = Arc::new(settings));
    }

    pub fn remna(&self) -> ServiceResult<Arc<dyn RemnawaveApi>> {
        self.read(|r| r.remna.clone()).ok_or_else(|| {
            ServiceError::RemnaNotConfigured("укажите адрес панели и API-токен в настройках".into())
        })
    }

    pub fn set_remna(&self, api: Option<Arc<dyn RemnawaveApi>>) {
        self.write(|r| r.remna = api);
    }

    pub fn remna_webhook_secret(&self) -> Option<String> {
        self.read(|r| r.remna_webhook_secret.clone())
    }

    pub fn set_remna_webhook_secret(&self, secret: Option<String>) {
        self.write(|r| r.remna_webhook_secret = secret);
    }

    pub fn bot_token(&self) -> Option<String> {
        self.read(|r| r.bot_token.clone())
    }

    pub fn set_bot_sender(&self, sender: Option<Arc<dyn BotSender>>) {
        self.write(|r| r.bot_sender = sender);
    }

    pub fn bot_sender(&self) -> Option<Arc<dyn BotSender>> {
        self.read(|r| r.bot_sender.clone())
    }

    pub fn bot_runtime(&self) -> &BotRuntime {
        &self.inner.bot
    }

    pub fn module_context(&self) -> ModuleContext {
        ModuleContext {
            store: self.inner.store.clone(),
            core: Arc::new(self.clone()),
            currency: self.settings().currency,
        }
    }

    pub async fn reload(&self) -> ServiceResult<()> {
        let mut conn = self.store().conn().await?;
        let mut core = settings::load_core(&mut conn).await?;
        if core.public_url.is_none() {
            core.public_url = self.inner.env.public_url.clone();
        }
        let remna = settings::remna_connection(&mut conn, self.secrets(), &self.inner.env).await?;
        let bot_token = settings::secret_or(
            &mut conn,
            self.secrets(),
            sn_domain::settings::secret_keys::BOT_TOKEN,
            self.inner.env.bot_token.clone(),
        )
        .await?;
        let snapshot = settings::registry_snapshot(
            &mut conn,
            self.secrets(),
            &self.inner.payments,
            &core,
            bot_token.as_deref(),
        )
        .await?;
        let bot_content = crate::bot_content::load(&mut conn).await?;
        drop(conn);
        self.inner.payments.apply(snapshot);
        self.inner.modules.set_currency(core.currency);
        self.inner.modules.load(self.store()).await?;
        let mut conn = self.store().conn().await?;
        let module_rows =
            settings::module_secret_rows(&mut conn, self.secrets(), &self.inner.modules).await?;
        drop(conn);
        self.inner.modules.apply_rows(&module_rows);
        let client: Option<Arc<dyn RemnawaveApi>> =
            if remna.url.is_empty() || remna.token.is_empty() {
                None
            } else {
                let mut config = RemnaConfig::new(&remna.url, &remna.token);
                config.cookie = remna.cookie.clone();
                match RemnawaveClient::new(config) {
                    Ok(client) => Some(Arc::new(GuardedRemnawave::new(client))),
                    Err(err) => {
                        tracing::warn!(error = %err, "подключение к Remnawave не настроено");
                        None
                    }
                }
            };
        let token_changed = self.bot_token() != bot_token;
        self.write(|r| {
            r.settings = Arc::new(core);
            r.remna = client;
            r.remna_webhook_secret = remna.webhook_secret;
            r.bot_token = bot_token;
            r.bot_content = Arc::new(bot_content);
        });
        if token_changed {
            self.inner.bot.wake();
        }
        Ok(())
    }

    pub fn auth(&self) -> Auth<'_> {
        Auth::new(self)
    }

    pub fn alerts(&self) -> crate::alerts::AlertsOps<'_> {
        crate::alerts::AlertsOps::new(self)
    }

    pub fn admins(&self) -> crate::admins::AdminsOps<'_> {
        crate::admins::AdminsOps::new(self)
    }

    pub fn api_keys(&self) -> crate::api_keys::ApiKeysOps<'_> {
        crate::api_keys::ApiKeysOps::new(self)
    }

    pub fn mail(&self) -> crate::mail::MailOps<'_> {
        crate::mail::MailOps::new(self)
    }

    pub fn clients(&self) -> ClientOps<'_> {
        ClientOps::new(self)
    }

    pub fn checkout(&self) -> Checkout<'_> {
        Checkout::new(self)
    }

    pub fn inbox(&self) -> PaymentInbox<'_> {
        PaymentInbox::new(self)
    }

    pub fn grants(&self) -> crate::grants::Grants<'_> {
        crate::grants::Grants::new(self)
    }

    pub fn fulfillment(&self) -> Fulfillment<'_> {
        Fulfillment::new(self)
    }

    pub fn subscriptions(&self) -> SubscriptionOps<'_> {
        SubscriptionOps::new(self)
    }

    pub fn remna_sync(&self) -> RemnaSync<'_> {
        RemnaSync::new(self)
    }

    pub fn autorenew(&self) -> AutoRenew<'_> {
        AutoRenew::new(self)
    }

    pub fn events(&self) -> EventDispatcher<'_> {
        EventDispatcher::new(self)
    }

    pub fn support(&self) -> SupportOps<'_> {
        SupportOps::new(self)
    }

    pub fn limits(&self) -> RateLimits<'_> {
        RateLimits::new(self)
    }

    pub fn admin(&self) -> AdminOps<'_> {
        AdminOps::new(self)
    }

    pub fn tariffs(&self) -> TariffAdmin<'_> {
        TariffAdmin::new(self)
    }

    pub fn update_config(&self) -> Option<Arc<crate::updates::UpdateConfig>> {
        self.read(|r| r.updates.clone())
    }

    pub fn set_update_config(&self, config: Option<crate::updates::UpdateConfig>) {
        self.write(|r| r.updates = config.map(Arc::new));
    }

    pub fn updates(&self) -> crate::updates::Updates<'_> {
        crate::updates::Updates::new(self)
    }

    pub fn infra(&self) -> InfraOps<'_> {
        InfraOps::new(self)
    }

    pub fn settings_ops(&self) -> SettingsOps<'_> {
        SettingsOps::new(self)
    }

    pub fn branding(&self) -> crate::branding::BrandingOps<'_> {
        crate::branding::BrandingOps::new(self)
    }

    pub fn landing(&self) -> crate::landing::LandingOps<'_> {
        crate::landing::LandingOps::new(self)
    }

    pub fn bot_content(&self) -> Arc<crate::bot_content::BotContent> {
        self.read(|r| r.bot_content.clone())
    }

    pub fn bot_content_ops(&self) -> crate::bot_content::BotContentOps<'_> {
        crate::bot_content::BotContentOps::new(self)
    }

    pub fn stars_review(&self) -> StarsReview<'_> {
        StarsReview::new(self)
    }
}
