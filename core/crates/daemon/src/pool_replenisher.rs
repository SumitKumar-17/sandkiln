//! Background task keeping every configured pool topped up toward its
//! `warm_count` — the other half of `crate::pool`. Same fixed-interval
//! tick-loop shape as `crate::idle_reaper`.

use crate::pool::PoolConfig;
use crate::routes_sandbox::{create_sandbox_core, CreateSandboxRequest};
use crate::routes_snapshot::{delete_snapshot_by_id, snapshot_and_stop, SnapshotStopError};
use crate::state::AppState;
use std::collections::HashMap;
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

pub async fn run(state: Arc<AppState>) {
    loop {
        tokio::time::sleep(CHECK_INTERVAL).await;
        replenish_once(&state).await;
    }
}

/// One slot per due pool per tick (not all at once), so a large
/// `warm_count` fills in gradually rather than spiking boot load; the
/// next tick continues. Configs are cloned out from under the lock before
/// the slow boot+snapshot work below, rather than holding it locked.
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
        // Neither egress nor env is baked into a warm-boot instance -- the
        // claimer's own values (if any) are applied fresh at claim time
        // instead, see `routes_sandbox::claim_from_pool`.
        egress: None,
        env: HashMap::new(),
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
            // Something else (direct API call, a concurrent idle-reaper
            // tick) stopped or snapshotted it first.
            tracing::warn!(pool_id = %config.id, sandbox_id = %sandbox_id, "warm instance was already gone by the time replenishment tried to snapshot it");
            return;
        }
        Err(SnapshotStopError::Blocked(_)) => {
            // Can't actually happen -- create_sandbox_core above never
            // jails/forks a warm-boot sandbox. Handled anyway to keep this
            // match exhaustive rather than a `_ =>` catch-all.
            tracing::warn!(pool_id = %config.id, sandbox_id = %sandbox_id, "warm instance was unexpectedly ineligible for snapshotting");
            return;
        }
        Err(SnapshotStopError::Io(e)) => {
            tracing::warn!(pool_id = %config.id, sandbox_id = %sandbox_id, error = %e, "failed to snapshot a freshly booted warm instance for pool replenishment");
            return;
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
}
