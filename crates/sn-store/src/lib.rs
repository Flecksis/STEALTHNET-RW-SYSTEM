pub mod constraints;
pub mod crypto;
pub mod db;
pub mod error;
pub mod repo;

#[cfg(any(test, feature = "test-util"))]
pub mod testing;

#[cfg(test)]
mod db_tests;

#[cfg(test)]
mod repo_tests;

pub use crypto::{SealedSecret, SecretBox};
pub use db::{Conn, Db, LockSpace, Store, StoreOptions, Tx};
pub use error::{StoreError, StoreResult};
pub use repo::*;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
