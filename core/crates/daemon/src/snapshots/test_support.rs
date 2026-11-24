//! Shared fixtures for the `snapshots` unit tests: a self-cleaning temp dir and
//! a throwaway `NetworkManager`, so `layout` and `held` tests do not
//! each carry their own copy (`retired` predates it and keeps its own).

use sandkiln_vmm::network::NetworkManager;
use std::fs;
use std::path::PathBuf;

/// Fresh, self-cleaning temp dir — real filesystem I/O, no mocking
/// (matches `sandkiln_vmm::drive`'s `TempStore` convention).
pub(crate) struct TempDir {
    pub(crate) path: PathBuf,
}

impl TempDir {
    pub(crate) fn new(test_name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("sandkiln-snapshot-test-{test_name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(crate) fn test_network(taps: impl IntoIterator<Item = String>) -> NetworkManager {
    NetworkManager::new("test-br0", "10.0.0.1".parse().unwrap(), "eth-test", taps)
}
