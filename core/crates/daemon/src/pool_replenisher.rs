//! Background task that keeps every configured pool topped up toward its
//! `warm_count` — the other half of `crate::pool`; see that module's doc
//! comment for the feature's overall shape and scope. Mirrors
//! `crate::idle_reaper`'s own tick-loop shape (a fixed-interval
//! `tokio::time::sleep`, reusing existing shared mechanics rather than
//! inventing new VM-lifecycle plumbing) since both are background tasks
//! that scan `AppState` and act on what they find.

use crate::pool::PoolConfig;
use crate::routes_sandbox::{create_sandbox_core, CreateSandboxRequest};
use crate::routes_snapshot::{delete_snapshot_by_id, snapshot_and_stop, SnapshotStopError};
use crate::state::AppState;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// How often the replenisher wakes to check every configured pool. Not
/// currently configurable — see `crate::pool`'s "scoped honestly" list;
/// a fixed default is enough for a first cut, and easy to turn into an
/// env var later if a real need to tune it shows up. Spawned
/// unconditionally from `main` (unlike `idle_reaper`, which only spawns
/// when a timeout is configured) since an idle tick with no pools
/// configured is a single empty lock-and-check, cheap enough not to
/// bother wiring conditional spawn logic for.
const CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// Tag applied to every warm-boot sandbox this creates, purely so it's
/// identifiable in `GET /sandboxes`/`ps` output mid-replenish — not read
/// back by any code here, and overwritten by the caller's own tags the
/// moment a claim actually resumes this snapshot (see
/// `routes_sandbox::create_sandbox_core`).
const POOL_TAG_KEY: &str = "sandkiln.pool";

pub async fn run(state: Arc<AppState>) {
    loop {
        tokio::time::sleep(CHECK_INTERVAL).await;
        replenish_once(&state).await;
    }
}

/// One tick: for every pool under its target, replenish exactly one slot
/// — not all of them at once, so a newly (re)configured pool with a large
/// `warm_count` fills in gradually rather than spiking boot load across
/// every tap device/CPU core at the same instant. The next tick picks up
/// where this one left off. Pool configs are cloned out under the lock
/// and acted on afterward, rather than holding `AppState::pools` locked
/// across the slow boot+snapshot work below.
async fn replenish_once(state: &Arc<AppState>) {
    let due: Vec<PoolConfig> = {
        let pools = state.pools.lock().unwrap();
        pools.values().filter(|p| p.needs_replenish()).map(|p| p.config.clone()).collect()
    };
    for config in due {
        replenish_one(state, config).await;
    }
}

async fn replenish_one(state: &Arc<AppState>, config: PoolConfig) {
    let request = CreateSandboxRequest {
        name: None,
        tags: HashMap::from([(POOL_TAG_KEY.to_string(), config.id.clone())]),
        drives: vec![],
        vcpu_count: Some(config.vcpu_count),
        mem_size_mib: Some(config.mem_size_mib),
        image_id: config.image_id.clone(),
        rate_limit: None,
    };
    let sandbox_id = match create_sandbox_core(state, request).await {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(pool_id = %config.id, error = %e, "failed to boot a warm instance for pool replenishment");
            return;
        }
    };

    let snapshot_id = match snapshot_and_stop(state.clone(), sandbox_id.clone()).await {
        Ok(id) => id,
        Err(SnapshotStopError::NotFound) => {
            // Only realistic cause: something else (an operator hitting
            // the HTTP API directly, a concurrent idle-reaper tick — this
            // sandbox is indistinguishable from any other by anything
            // outside this pool) stopped or snapshotted it first.
            tracing::warn!(pool_id = %config.id, sandbox_id = %sandbox_id, "warm instance was already gone by the time replenishment tried to snapshot it");
            return;
        }
        Err(SnapshotStopError::Blocked(_)) => {
            // Can't actually happen: `create_sandbox_core` above never
            // jails or forks a warm-boot sandbox. Handled anyway so this
            // match stays exhaustive against `SnapshotStopError`'s real
            // shape rather than a `_ =>` catch-all.
            tracing::warn!(pool_id = %config.id, sandbox_id = %sandbox_id, "warm instance was unexpectedly ineligible for snapshotting");
            return;
        }
        Err(SnapshotStopError::Io(e)) => {
            tracing::warn!(pool_id = %config.id, sandbox_id = %sandbox_id, error = %e, "failed to snapshot a freshly booted warm instance for pool replenishment");
            return;
        }
    };

    // The pool may have been deleted (the only way to reconfigure one —
    // see `routes_pool`) while the boot+snapshot above was in flight; if
    // so, this snapshot has nowhere to go and is cleaned up rather than
    // left orphaned on disk with no pool ever able to claim it.
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
}
