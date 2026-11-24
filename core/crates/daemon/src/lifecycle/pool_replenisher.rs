//! Background task keeping every configured pool topped up toward its
//! `warm_count` — the other half of `crate::lifecycle::pool`. Same fixed-interval
//! tick-loop shape as `crate::lifecycle::idle_reaper`.

use crate::lifecycle::pool::PoolConfig;
use crate::http::routes::sandbox::CreateSandboxRequest;
use crate::lifecycle::create::create_warm_instance;
use crate::snapshots::ops::delete::delete_snapshot_by_id;
use crate::snapshots::ops::take::{snapshot_and_stop, SnapshotStopError};
use crate::state::AppState;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Not configurable yet — a fixed default is enough for a first cut.
/// Spawned unconditionally from `main`: an idle tick with no pools
/// configured is one cheap empty lock-and-check.
const CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// Tags a warm-boot sandbox for identifiability in `GET /sandboxes`
/// mid-replenish — not read back here, overwritten by the claimer's own
/// tags once a claim resumes this snapshot.
const POOL_TAG_KEY: &str = "sandkiln.pool";

/// Guest addresses a background replenish must leave free for real users:
/// every warm instance holds one for as long as it waits.
const RESERVED_GUEST_ADDRESSES: usize = 16;

/// Free space under `state_dir` below which no new warm snapshot is written
/// (each is a full-RAM file).
const MIN_FREE_DISK_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Why background replenishment should pause now, if it should. Pure so the
/// thresholds are unit-tested; the caller supplies what the host reports.
pub(crate) fn replenish_blocker(free_addresses: usize, free_taps: usize, free_disk_bytes: Option<u64>) -> Option<&'static str> {
    if free_taps == 0 {
        return Some("no free tap device");
    }
    if free_addresses <= RESERVED_GUEST_ADDRESSES {
        return Some("guest addresses are running low");
    }
    match free_disk_bytes {
        Some(free) if free < MIN_FREE_DISK_BYTES => Some("disk space is running low"),
        _ => None,
    }
}

/// Logs the transition into and out of a blocked state once, not every tick.
static PAUSED: AtomicBool = AtomicBool::new(false);

/// How many warm instances one pool boots at the same time. Boot and snapshot
/// are CPU and disk heavy; two overlap well without starving real creates.
const MAX_PARALLEL_BOOTS: usize = 2;

/// How many boots to start for a pool short by `deficit` instances.
pub(crate) fn boots_to_launch(deficit: usize) -> usize {
    deficit.min(MAX_PARALLEL_BOOTS)
}

/// Runs until the process exits. Wakes on the fixed tick and, sooner, whenever
/// a claim drains a pool or a pool is created (`AppState::replenish_wake`), so
/// a claimed warm instance is replaced right away instead of up to a tick
/// later. Keeps going while it is making progress.
pub async fn run(state: Arc<AppState>) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(CHECK_INTERVAL) => {}
            _ = state.replenish_wake.notified() => {}
        }
        while replenish_once(&state).await > 0 {}
    }
}

/// One slot per due pool per tick (not all at once), so a large
/// `warm_count` fills in gradually rather than spiking boot load; the
/// next tick continues. Configs are cloned out from under the lock before
/// the slow boot+snapshot work below, rather than holding it locked.
async fn replenish_once(state: &Arc<AppState>) -> usize {
    let free_disk = crate::http::routes::status::disk_usage(&state.config.state_dir).map(|d| d.free_bytes);
    let blocker = replenish_blocker(state.network.free_ip_count(), state.network.free_tap_count(), free_disk);
    if PAUSED.swap(blocker.is_some(), Ordering::Relaxed) != blocker.is_some() {
        match blocker {
            Some(why) => tracing::warn!(reason = why, "pausing warm pool replenishment"),
            None => tracing::info!("resuming warm pool replenishment"),
        }
    }
    if blocker.is_some() {
        return 0;
    }
    let due: Vec<(PoolConfig, usize)> = {
        let pools = state.pools.lock().unwrap();
        pools.values().filter(|p| p.needs_replenish()).map(|p| (p.config.clone(), boots_to_launch(p.warm_deficit()))).collect()
    };
    let boots = due.into_iter().flat_map(|(config, n)| std::iter::repeat_n(config, n)).map(|config| replenish_one(state, config));
    futures_util::future::join_all(boots).await.into_iter().filter(|ok| *ok).count()
}

/// Boots, snapshots and enqueues one warm instance. `true` when it ended up
/// warm in a still-configured pool.
async fn replenish_one(state: &Arc<AppState>, config: PoolConfig) -> bool {
    let request = CreateSandboxRequest {
        name: None,
        tags: HashMap::from([(POOL_TAG_KEY.to_string(), config.id.clone())]),
        drives: vec![],
        vcpu_count: Some(config.vcpu_count),
        mem_size_mib: Some(config.mem_size_mib),
        image_id: config.image_id.clone(),
        rate_limit: None,
        // Neither egress nor env is baked into a warm-boot instance -- the
        // claimer's own values (if any) are applied fresh at claim time
        // instead, see `lifecycle::pool_claim::claim_from_pool`.
        egress: None,
        env: HashMap::new(),
    };
    let sandbox_id = match create_warm_instance(state, request).await {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(pool_id = %config.id, error = %e, "failed to boot a warm instance for pool replenishment");
            return false;
        }
    };

    let snapshot_id = match snapshot_and_stop(state.clone(), sandbox_id.clone()).await {
        Ok(id) => id,
        Err(SnapshotStopError::NotFound) => {
            // Something else (direct API call, a concurrent idle-reaper
            // tick) stopped or snapshotted it first.
            tracing::warn!(pool_id = %config.id, sandbox_id = %sandbox_id, "warm instance was already gone by the time replenishment tried to snapshot it");
            return false;
        }
        Err(SnapshotStopError::Blocked(_)) => {
            // Can't actually happen -- create_sandbox_core above never
            // jails/forks a warm-boot sandbox. Handled anyway to keep this
            // match exhaustive rather than a `_ =>` catch-all.
            tracing::warn!(pool_id = %config.id, sandbox_id = %sandbox_id, "warm instance was unexpectedly ineligible for snapshotting");
            return false;
        }
        Err(SnapshotStopError::Io(e)) => {
            tracing::warn!(pool_id = %config.id, sandbox_id = %sandbox_id, error = %e, "failed to snapshot a freshly booted warm instance for pool replenishment");
            return false;
        }
    };

    // Pool may have been deleted while the boot+snapshot above was in
    // flight; if so, clean up rather than leave the snapshot orphaned.
    let still_configured = {
        let mut pools = state.pools.lock().unwrap();
        match pools.get_mut(&config.id) {
            Some(pool) => {
                pool.push_warm(snapshot_id.clone());
                true
            }
            None => false,
        }
    };
    if still_configured {
        tracing::info!(pool_id = %config.id, sandbox_id = %sandbox_id, snapshot_id = %snapshot_id, "replenished one warm instance for pool");
    } else {
        tracing::info!(pool_id = %config.id, snapshot_id = %snapshot_id, "pool was deleted while a warm instance was being prepared for it — cleaning up rather than leaving it orphaned");
        if let Err(e) = delete_snapshot_by_id(state.clone(), snapshot_id).await {
            tracing::warn!(pool_id = %config.id, error = %e, "failed to clean up an orphaned warm snapshot from a deleted pool");
        }
    }
    still_configured
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pool_boots_at_most_two_instances_at_once() {
        assert_eq!((boots_to_launch(0), boots_to_launch(1), boots_to_launch(2), boots_to_launch(9)), (0, 1, 2, 2));
    }

    #[test]
    fn replenishment_runs_when_the_host_has_headroom() {
        assert_eq!(replenish_blocker(200, 20, Some(50 << 30)), None);
        assert_eq!(replenish_blocker(200, 20, None), None, "an unreadable disk does not block");
    }

    #[test]
    fn each_shortage_pauses_replenishment_with_its_own_reason() {
        assert_eq!(replenish_blocker(200, 0, Some(50 << 30)), Some("no free tap device"));
        assert_eq!(replenish_blocker(RESERVED_GUEST_ADDRESSES, 20, Some(50 << 30)), Some("guest addresses are running low"));
        assert_eq!(replenish_blocker(200, 20, Some(1 << 30)), Some("disk space is running low"));
    }
}
