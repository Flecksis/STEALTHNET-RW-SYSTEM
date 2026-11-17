use crate::config::ServerConfig;
use sn_services::Services;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<ServerConfig>,
    pub services: Services,
}

impl AppState {
    pub fn new(config: ServerConfig, services: Services) -> Self {
        AppState {
            config: Arc::new(config),
            services,
        }
    }
}
