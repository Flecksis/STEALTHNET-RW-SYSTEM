mod address;
mod check;
mod client;
mod handle;
mod plan;

pub use address::{CREDENTIAL_MAX, Proxy, ProxyError, ProxyServer, Scheme, outward};
pub use check::{Failure, PROBE_TIMEOUT, Problem, Reached, diagnose, explain, probe};
pub use client::{Http, Profile, build, routed};
pub use handle::{Egress, current};
pub use plan::{EnvError, EnvProxy, Plan, ProxySettings, Route, Source};

#[cfg(test)]
mod tests;
