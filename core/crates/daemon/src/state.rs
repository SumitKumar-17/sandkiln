use crate::config::Config;
use crate::sandbox::Sandbox;
use crate::snapshot::Snapshot;
use sandkiln_vmm::drive::DriveStore;
use sandkiln_vmm::network::NetworkManager;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct AppState {
    pub config: Config,
    pub network: NetworkManager,
    pub drives: DriveStore,
    pub sandboxes: Mutex<HashMap<String, Sandbox>>,
    pub snapshots: Mutex<HashMap<String, Snapshot>>,
}

impl AppState {
    pub fn new(config: Config, network: NetworkManager, drives: DriveStore) -> Self {
        Self {
            config,
            network,
            drives,
            sandboxes: Mutex::new(HashMap::new()),
            snapshots: Mutex::new(HashMap::new()),
        }
    }
}
