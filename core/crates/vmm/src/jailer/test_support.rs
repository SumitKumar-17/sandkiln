//! Test-only helpers shared by the jailer submodules' unit tests.

use std::fs;
use std::path::PathBuf;

pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new(test_name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("sandkiln-jailer-test-{test_name}-{}", std::process::id()));
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
