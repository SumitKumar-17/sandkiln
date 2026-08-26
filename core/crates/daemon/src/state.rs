use crate::config::Config;
use crate::sandbox::Sandbox;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct AppState {
    pub config: Config,
    pub sandboxes: Mutex<HashMap<String, Sandbox>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self { config, sandboxes: Mutex::new(HashMap::new()) }
    }
}
