pub mod address;
pub mod api;
pub mod client;
pub mod error;
pub mod guard;
pub mod models;
pub mod webhook;

#[cfg(any(test, feature = "fake"))]
pub mod fake;

pub use api::RemnawaveApi;
pub use client::{RemnaConfig, RemnawaveClient};
pub use error::{RemnaError, RemnaResult};
pub use guard::{BreakerConfig, BreakerState, GuardedRemnawave};
pub use models::*;

pub const SUPPORTED_VERSION: &str = "3.4.11";
pub const BULK_LIMIT: usize = 500;
