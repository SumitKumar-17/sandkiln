//! `GET /status`: one JSON document answering "is this host healthy, and
//! what is it doing?" — the operator's counterpart to `/metrics`.
//!
//! Behind the bearer token (unlike `/metrics`) because it names host paths
//! and capacity. The pure parts (`disk_usage`'s thresholds, `warnings`) are
//! split out so they are unit-tested without a daemon.

use crate::metrics::PoolClaimOutcome;
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

/// Tap devices: needed only while a guest runs, so `in_use` is the number of
/// running networked sandboxes (forks included) — held snapshots are not in it.
#[derive(Serialize)]
pub struct TapPool {
    pub size: usize,
    pub free: usize,
    pub in_use: usize,
}

/// Guest IPs in the sandbox /24: held by every running sandbox *and* every
/// held snapshot (the address is frozen into its memory image), so this is the
/// pool that bounds how many snapshots can exist.
#[derive(Serialize)]
pub struct IpPool {
    pub size: usize,
    pub free: usize,
    pub held_by_snapshots: usize,
}

/// Creates that matched a pool, by outcome, since the daemon started.
#[derive(Serialize)]
pub struct PoolClaims {
    pub warm: u64,
    pub warm_failed: u64,
    pub cold_slot: u64,
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
    pub ip_pool: IpPool,
    pub sandboxes_created_total: u64,
    pub rootfs_clones: RootfsClones,
    pub pool_claims: PoolClaims,
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

/// Free capacity below which a pool is "nearly exhausted": a tenth of it, at
/// least one slot.
fn is_low(free: usize, size: usize) -> bool {
    size > 0 && free > 0 && free <= (size / 10).max(1)
}

/// Conditions an operator should act on, as plain sentences.
pub fn warnings(taps: &TapPool, ips: &IpPool, disks: &[DiskUsage], clones: &RootfsClones, auth_enabled: bool) -> Vec<String> {
    let mut out = Vec::new();
    if taps.size > 0 && taps.free == 0 {
        out.push(
            "no free tap devices: every tap is in use by a running sandbox, so creating or resuming one fails (503) until a running sandbox stops"
                .to_string(),
        );
    } else if is_low(taps.free, taps.size) {
        out.push(format!("tap pool nearly exhausted: {} of {} free (one is needed per running sandbox)", taps.free, taps.size));
    }
    if ips.free == 0 {
        out.push(format!(
            "no free sandbox IPs: all {} are held, {} of them by snapshots; delete snapshots you no longer need",
            ips.size, ips.held_by_snapshots
        ));
    } else if is_low(ips.free, ips.size) {
        out.push(format!(
            "sandbox IPs running low: {} of {} free, {} held by snapshots (each snapshot keeps its IP); delete snapshots you no longer need",
            ips.free, ips.size, ips.held_by_snapshots
        ));
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
    let tap_size = state.network.tap_pool_size();
    let tap_free = state.network.free_tap_count();
    let tap_pool = TapPool { size: tap_size, free: tap_free, in_use: tap_size.saturating_sub(tap_free) };
    let ip_pool = IpPool {
        size: state.network.ip_pool_size(),
        free: state.network.free_ip_count(),
        held_by_snapshots: counts.held_snapshots,
    };
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
    let warnings = warnings(&tap_pool, &ip_pool, &disk, &clones, auth_enabled);
    Json(Status {
        version: env!("CARGO_PKG_VERSION"),
        uptime_secs: state.started_at.elapsed().as_secs(),
        auth_enabled,
        jailer_enabled: state.config.jailer.is_some(),
        counts,
        tap_pool,
        ip_pool,
        sandboxes_created_total: state.metrics.sandboxes_created(),
        rootfs_clones: clones,
        pool_claims: PoolClaims {
            warm: state.metrics.pool_claim_count(PoolClaimOutcome::Warm),
            warm_failed: state.metrics.pool_claim_count(PoolClaimOutcome::WarmFailed),
            cold_slot: state.metrics.pool_claim_count(PoolClaimOutcome::ColdSlot),
        },
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

    fn taps(size: usize, free: usize) -> TapPool {
        TapPool { size, free, in_use: size - free }
    }

    fn ips(free: usize, held_by_snapshots: usize) -> IpPool {
        IpPool { size: 253, free, held_by_snapshots }
    }

    fn healthy() -> (TapPool, IpPool) {
        (taps(8, 3), ips(200, 10))
    }

    #[test]
    fn healthy_host_has_no_warnings() {
        let (t, i) = healthy();
        assert!(warnings(&t, &i, &[disk(50)], &RootfsClones { reflink: 4, copy: 0 }, true).is_empty());
    }

    #[test]
    fn exhausted_tap_pool_is_flagged_but_an_unconfigured_one_is_not() {
        let (_, i) = healthy();
        let w = warnings(&taps(8, 0), &i, &[], &NO_CLONES, true);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("running sandbox"), "{w:?}");
        assert!(warnings(&taps(0, 0), &i, &[], &NO_CLONES, true).is_empty());
    }

    #[test]
    fn a_nearly_full_tap_pool_warns_before_it_is_empty() {
        let (_, i) = healthy();
        let w = warnings(&taps(32, 4), &i, &[], &NO_CLONES, true);
        assert!(w.is_empty(), "4 of 32 free is not yet low: {w:?}");
        let w = warnings(&taps(32, 3), &i, &[], &NO_CLONES, true);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("nearly exhausted"), "{w:?}");
    }

    #[test]
    fn low_and_empty_ip_pools_say_how_many_snapshots_hold_them() {
        let (t, _) = healthy();
        let low = warnings(&t, &ips(25, 200), &[], &NO_CLONES, true);
        assert_eq!(low.len(), 1);
        assert!(low[0].contains("200 held by snapshots"), "{low:?}");
        let none = warnings(&t, &ips(0, 253), &[], &NO_CLONES, true);
        assert_eq!(none.len(), 1);
        assert!(none[0].contains("no free sandbox IPs"), "{none:?}");
        assert!(warnings(&t, &ips(26, 100), &[], &NO_CLONES, true).is_empty());
    }

    /// The scenario this pool split exists for: many snapshots, plenty of taps.
    #[test]
    fn many_held_snapshots_alone_do_not_warn_about_taps() {
        let w = warnings(&taps(32, 32), &ips(253 - 100, 100), &[], &NO_CLONES, true);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn low_disk_is_flagged_per_volume() {
        let (t, i) = healthy();
        let w = warnings(&t, &i, &[disk(50), disk(2)], &NO_CLONES, true);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("2048 MiB"), "{w:?}");
    }

    #[test]
    fn copy_only_cloning_and_open_auth_are_flagged() {
        let (t, i) = healthy();
        let w = warnings(&t, &i, &[], &RootfsClones { reflink: 0, copy: 3 }, false);
        assert_eq!(w.len(), 2);
        // Mixed methods (a one-off fallback) is not a configuration problem.
        assert!(warnings(&t, &i, &[], &RootfsClones { reflink: 5, copy: 1 }, true).is_empty());
    }

    #[test]
    fn disk_usage_reads_a_real_directory_and_rejects_a_missing_one() {
        let d = disk_usage(&std::env::temp_dir()).expect("temp dir has a filesystem");
        assert!(d.total_bytes > 0 && d.free_bytes <= d.total_bytes);
        assert!(disk_usage(Path::new("/definitely/not/here")).is_none());
    }
}
