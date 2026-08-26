use crate::config::Config;
use crate::sandbox::Sandbox;
use sandkiln_vmm::network::NetworkManager;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct AppState {
    pub config: Config,
    pub network: NetworkManager,
    pub sandboxes: Mutex<HashMap<String, Sandbox>>,
}

impl AppState {
    pub fn new(config: Config, network: NetworkManager) -> Self {
        Self { config, network, sandboxes: Mutex::new(HashMap::new()) }
    }
}
