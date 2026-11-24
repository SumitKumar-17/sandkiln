//! `GET /status`: one JSON document answering "is this host healthy, and
//! what is it doing?" — the operator's counterpart to `/metrics`.
//!
//! Behind the bearer token (unlike `/metrics`) because it names host paths
//! and capacity. The pure parts (`disk_usage`'s thresholds, `warnings`) are
//! split out so they are unit-tested without a daemon.

use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;

/// Below this much free space on a state or rootfs volume the status
/// carries a warning: a full-RAM checkpoint is `mem_size_mib` on its own.
const LOW_DISK_BYTES: u64 = 5 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiskUsage {
    pub path: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Serialize)]
pub struct Counts {
    pub sandboxes: usize,
    pub held_snapshots: usize,
    pub retired_checkpoints: usize,
    pub pools: usize,
    pub pool_warm_ready: usize,
}

#[derive(Serialize)]
pub struct TapPool {
    pub size: u32,
    pub free: usize,
}

#[derive(Serialize)]
pub struct RootfsClones {
    pub reflink: u64,
    pub copy: u64,
}

#[derive(Serialize)]
pub struct Status {
    pub version: &'static str,
    pub uptime_secs: u64,
    pub auth_enabled: bool,
    pub jailer_enabled: bool,
    pub counts: Counts,
    pub tap_pool: TapPool,
    pub sandboxes_created_total: u64,
    pub rootfs_clones: RootfsClones,
    pub disk: Vec<DiskUsage>,
    pub warnings: Vec<String>,
}

/// Free/total bytes of the filesystem holding `path`, `None` if it cannot
/// be read (missing directory, permission).
pub fn disk_usage(path: &Path) -> Option<DiskUsage> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid NUL-terminated string and `st` a valid out-pointer.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let frag = st.f_frsize as u64;
    Some(DiskUsage {
        path: path.display().to_string(),
        total_bytes: st.f_blocks as u64 * frag,
        free_bytes: st.f_bavail as u64 * frag,
    })
}

/// Conditions an operator should act on, as plain sentences.
pub fn warnings(tap_free: usize, tap_size: u32, disks: &[DiskUsage], clones: &RootfsClones, auth_enabled: bool) -> Vec<String> {
    let mut out = Vec::new();
    if tap_size > 0 && tap_free == 0 {
        out.push("no free tap devices: new networked sandboxes will fail until one is stopped".to_string());
    }
    for d in disks {
        if d.free_bytes < LOW_DISK_BYTES {
            out.push(format!(
                "low disk space on {}: {} MiB free (snapshots and clones need room)",
                d.path,
                d.free_bytes / (1024 * 1024)
            ));
        }
    }
    if clones.copy > 0 && clones.reflink == 0 {
        out.push("every rootfs clone was a full byte copy: put rootfs_dir on a reflink filesystem (XFS/Btrfs) for ~10x faster create".to_string());
    }
    if !auth_enabled {
        out.push("auth_token is unset: the API is open to anyone who can reach it".to_string());
    }
    out
}

pub async fn status(State(state): State<Arc<AppState>>) -> Json<Status> {
    // One lock per statement: a std `Mutex` guard lives to the end of the
    // statement, so locking `pools` twice in one expression self-deadlocks.
    let (pools, pool_warm_ready) = {
        let pools = state.pools.lock().unwrap();
        (pools.len(), pools.values().map(|p| p.warm_ready()).sum())
    };
    let counts = Counts {
        sandboxes: state.sandboxes.lock().unwrap().len(),
        held_snapshots: state.snapshots.lock().unwrap().len(),
        retired_checkpoints: state.retired_snapshots.lock().unwrap().len(),
        pools,
        pool_warm_ready,
    };
    let tap_free = state.network.free_tap_devices().len();
    let (reflink, copy) = state.metrics.rootfs_clone_counts();
    let clones = RootfsClones { reflink, copy };
    let mut disk: Vec<DiskUsage> = Vec::new();
    for dir in [&state.config.state_dir, &state.config.rootfs_dir] {
        if let Some(d) = disk_usage(dir) {
            if !disk.iter().any(|seen| seen.total_bytes == d.total_bytes && seen.free_bytes == d.free_bytes && seen.path == d.path) {
                disk.push(d);
            }
        }
    }
    let auth_enabled = state.config.auth_token.is_some();
    let warnings = warnings(tap_free, state.config.tap_pool_size, &disk, &clones, auth_enabled);
    Json(Status {
        version: env!("CARGO_PKG_VERSION"),
        uptime_secs: state.started_at.elapsed().as_secs(),
        auth_enabled,
        jailer_enabled: state.config.jailer.is_some(),
        counts,
        tap_pool: TapPool { size: state.config.tap_pool_size, free: tap_free },
        sandboxes_created_total: state.metrics.sandboxes_created(),
        rootfs_clones: clones,
        disk,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(free_gib: u64) -> DiskUsage {
        DiskUsage { path: "/x".into(), total_bytes: 100 << 30, free_bytes: free_gib << 30 }
    }
    const NO_CLONES: RootfsClones = RootfsClones { reflink: 0, copy: 0 };

    #[test]
    fn healthy_host_has_no_warnings() {
        assert!(warnings(3, 8, &[disk(50)], &RootfsClones { reflink: 4, copy: 0 }, true).is_empty());
    }

    #[test]
    fn exhausted_tap_pool_is_flagged_but_an_unconfigured_one_is_not() {
        assert_eq!(warnings(0, 8, &[], &NO_CLONES, true).len(), 1);
        assert!(warnings(0, 0, &[], &NO_CLONES, true).is_empty());
    }

    #[test]
    fn low_disk_is_flagged_per_volume() {
        let w = warnings(1, 1, &[disk(50), disk(2)], &NO_CLONES, true);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("2048 MiB"), "{w:?}");
    }

    #[test]
    fn copy_only_cloning_and_open_auth_are_flagged() {
        let w = warnings(1, 1, &[], &RootfsClones { reflink: 0, copy: 3 }, false);
        assert_eq!(w.len(), 2);
        // Mixed methods (a one-off fallback) is not a configuration problem.
        assert!(warnings(1, 1, &[], &RootfsClones { reflink: 5, copy: 1 }, true).is_empty());
    }

    #[test]
    fn disk_usage_reads_a_real_directory_and_rejects_a_missing_one() {
        let d = disk_usage(&std::env::temp_dir()).expect("temp dir has a filesystem");
        assert!(d.total_bytes > 0 && d.free_bytes <= d.total_bytes);
        assert!(disk_usage(Path::new("/definitely/not/here")).is_none());
    }
}
