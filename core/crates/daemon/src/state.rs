use crate::config::Config;
use crate::metrics::Metrics;
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
    pub metrics: Metrics,
}

impl AppState {
    pub fn new(config: Config, network: NetworkManager, drives: DriveStore) -> Self {
        Self {
            config,
            network,
            drives,
            sandboxes: Mutex::new(HashMap::new()),
            snapshots: Mutex::new(HashMap::new()),
            metrics: Metrics::new(),
        }
    }

    /// Where a drive id is currently held, if anywhere — a running
    /// sandbox, or a snapshot with it frozen into saved state (Firecracker
    /// bakes a drive's host path into the snapshot the same way it does
    /// network config, so a snapshotted drive is still "in use" even
    /// though no `Vm` is running). Checked wherever an operation would
    /// conflict with the drive still being held: attaching it to another
    /// sandbox, or deleting it outright.
    pub fn drive_holder(&self, drive_id: &str) -> Option<String> {
        if let Some(sandbox) = self.sandboxes.lock().unwrap().values().find(|s| s.attached_drives.iter().any(|d| d == drive_id))
        {
            return Some(format!("sandbox {}", sandbox.id));
        }
        if let Some(snapshot) =
            self.snapshots.lock().unwrap().values().find(|s| s.attached_drives.iter().any(|d| d == drive_id))
        {
            return Some(format!("snapshot {}", snapshot.id));
        }
        None
    }
}
