pub mod catalog;
pub mod context;
pub mod error;
pub mod gate;
pub mod module;
pub mod placeholder;
pub mod registry;

pub use context::{BalanceChange, CoreApi, ModuleContext, Notification};
pub use error::ModuleError;
pub use module::{BotEntry, ItemHook, Module, ModuleConfig, QuoteHook, QuoteOrigin, ScheduledTask};
pub use placeholder::Placeholder;
pub use registry::{ModuleRegistry, ModuleView};
pub use sn_domain::module::{CATALOG, CoreFeature, MenuEntry, ModuleGroup, ModuleId, ModuleInfo};
