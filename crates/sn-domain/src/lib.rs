pub mod access;
pub mod client;
pub mod error;
pub mod events;
pub mod format;
pub mod ids;
pub mod jobs;
pub mod ledger;
pub mod module;
pub mod money;
pub mod order;
pub mod paging;
pub mod payment;
pub mod plan;
pub mod pricing;
pub mod settings;
pub mod subscription;
pub mod support;
pub mod tariff;
pub mod traffic;

pub use error::DomainError;
pub use ids::*;
pub use money::{Amount, Currency, Money, PayCurrency, Stars};

pub type DomainResult<T> = Result<T, DomainError>;
