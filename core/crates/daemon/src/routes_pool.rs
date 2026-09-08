//! HTTP handlers for pre-warmed pool configuration — creating, listing,
//! and deleting a pool's *configuration*. The actual replenishment
//! (booting and snapshotting warm instances in the background) lives in
//! `crate::pool_replenisher`; the actual claiming (a matching
//! `POST /sandboxes` resuming a warm snapshot instead of cold-booting)
//! lives in `routes_sandbox::create_sandbox_core`. See `crate::pool`'s
//! module doc comment for the feature's overall shape and scope.

use crate::error::AppError;
use crate::pool::{Pool, PoolConfig};
use crate::routes_sandbox::resolve_resource_override;
use crate::routes_snapshot::delete_snapshot_by_id;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct CreatePoolRequest {
    /// Caller-given identity, unique among currently configured pools —
    /// `409` if already taken, same convention as `POST /images`. Not
    /// exposed to the guest or a sandbox created from this pool in any
    /// way; purely how a caller refers back to this configuration.
    pub id: String,
    /// Boots warm instances from this registered image (see
    /// `POST /images`) instead of the daemon's `SANDKILN_BASE_ROOTFS`
    /// default. Same semantics as `CreateSandboxRequest::image_id` — a
    /// `POST /sandboxes` only ever matches a pool whose `image_id` is
    /// exactly the same as its own (both `None` counts as a match).
    #[serde(default)]
    pub image_id: Option<String>,
    #[serde(default)]
    pub vcpu_count: Option<u8>,
    #[serde(default)]
    pub mem_size_mib: Option<u32>,
    /// How many resumable snapshots to keep ready at once. `0` is valid
    /// (configures the pool's identity/matching key without ever keeping
    /// anything warm) but a strange thing to actually want — accepted
    /// rather than rejected since there's no *incorrect* behavior it
    /// would cause, just an inert pool.
    pub warm_count: u32,
    /// Maximum number of live instances (warm + claimed, combined) this
    /// pool's profile may ever have at once. Omitted means unbounded — a
    /// claim past what's warm always just cold-creates, the original
    /// behavior from before this field existed. When set, a claim that
    /// arrives at the ceiling queues (up to `routes_sandbox::POOL_QUEUE_TIMEOUT`)
    /// instead of either rejecting it or exceeding the ceiling.
    #[serde(default)]
    pub max_count: Option<u32>,
}

#[derive(Serialize)]
pub struct PoolSummary {
    id: String,
    image_id: Option<String>,
    vcpu_count: u8,
    mem_size_mib: u32,
    warm_count: u32,
    max_count: Option<u32>,
    /// How many resumable snapshots are actually sitting warm right now
    /// — can be less than `warm_count` right after the pool is created
    /// or a claim just drained it; `crate::pool_replenisher` tops it back
    /// up in the background, not instantly.
    warm_ready: usize,
    /// How many live instances of this pool's profile exist right now
    /// (a resumed warm claim or a cold-created match, either way) —
    /// tracked regardless of whether `max_count` is set, since it's
    /// useful visibility on its own; only *enforced against* when
    /// `max_count` is set.
    claimed: u32,
}

fn summarize(pool: &Pool) -> PoolSummary {
    PoolSummary {
        id: pool.config.id.clone(),
        image_id: pool.config.image_id.clone(),
        vcpu_count: pool.config.vcpu_count,
        mem_size_mib: pool.config.mem_size_mib,
        warm_count: pool.config.warm_count,
        max_count: pool.config.max_count,
        warm_ready: pool.warm_ready(),
        claimed: pool.claimed_count(),
    }
}

#[tracing::instrument(skip(state, request))]
pub async fn create_pool(State(state): State<Arc<AppState>>, Json(request): Json<CreatePoolRequest>) -> Result<Json<PoolSummary>, AppError> {
    if request.id.is_empty() {
        return Err(AppError::BadRequest("pool id must not be empty".to_string()));
    }
    if let Some(image_id) = &request.image_id {
        if !state.images.exists(image_id) {
            return Err(AppError::ImageNotFound(image_id.clone()));
        }
    }
    let vcpu_count = resolve_resource_override(request.vcpu_count, state.config.vcpu_count, state.config.max_vcpu_count, "vcpu_count")
        .map_err(AppError::BadRequest)?;
    let mem_size_mib =
        resolve_resource_override(request.mem_size_mib, state.config.mem_size_mib, state.config.max_mem_size_mib, "mem_size_mib")
            .map_err(AppError::BadRequest)?;

    if let Some(0) = request.max_count {
        return Err(AppError::BadRequest("max_count must be greater than 0 if given at all — omit it for an unbounded pool".to_string()));
    }
    let config = PoolConfig {
        id: request.id.clone(),
        image_id: request.image_id,
        vcpu_count,
        mem_size_mib,
        warm_count: request.warm_count,
        max_count: request.max_count,
    };

    let mut pools = state.pools.lock().unwrap();
    if pools.contains_key(&request.id) {
        return Err(AppError::Conflict(format!("pool {} already exists — delete it first to reconfigure it", request.id)));
    }
    pools.insert(request.id.clone(), Pool::new(config));
    Ok(Json(summarize(pools.get(&request.id).expect("just inserted"))))
}

#[derive(Serialize)]
pub struct ListPoolsResponse {
    pools: Vec<PoolSummary>,
}

pub async fn list_pools(State(state): State<Arc<AppState>>) -> Json<ListPoolsResponse> {
    let pools = state.pools.lock().unwrap();
    Json(ListPoolsResponse { pools: pools.values().map(summarize).collect() })
}

/// Removes a pool's configuration and destroys whatever it currently has
/// warm — `crate::pool_replenisher` naturally stops topping it up once
/// it's gone from `AppState::pools`, so nothing further to signal there.
/// A pool with nothing warm right now deletes just as cleanly (the loop
/// below is simply empty). Wakes anything queued on this pool (see
/// `routes_sandbox::create_sandbox_core`'s `max_count` queueing) one
/// last time so a queued caller notices the pool is gone and fails
/// clearly, instead of waiting out its own timeout for nothing.
#[tracing::instrument(skip(state))]
pub async fn delete_pool(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    let mut pool = { state.pools.lock().unwrap().remove(&id).ok_or_else(|| AppError::NotFound(id.clone()))? };
    pool.notify.notify_waiters();
    while let Some(snapshot_id) = pool.take_warm() {
        if let Err(e) = delete_snapshot_by_id(state.clone(), snapshot_id.clone()).await {
            tracing::warn!(pool_id = %id, snapshot_id = %snapshot_id, error = %e, "failed to clean up a warm snapshot while deleting its pool");
        }
    }
    Ok(StatusCode::NO_CONTENT)
}
