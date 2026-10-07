//! HTTP handlers for pool *configuration* only — create/list/delete.
//! Replenishment lives in `crate::pool_replenisher`, claiming in
//! `crate::pool_claim` (invoked from `routes_sandbox::create_sandbox_core`).
//! See `crate::pool`'s doc comment for the feature's overall shape.

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
    /// Unique among configured pools — `409` if taken, same convention as
    /// `POST /images`. Never exposed to the guest; purely a caller handle.
    pub id: String,
    /// Boots warm instances from this registered image instead of
    /// `SANDKILN_BASE_ROOTFS`. A `POST /sandboxes` only matches a pool
    /// with the exact same `image_id` (both `None` counts as a match).
    #[serde(default)]
    pub image_id: Option<String>,
    #[serde(default)]
    pub vcpu_count: Option<u8>,
    #[serde(default)]
    pub mem_size_mib: Option<u32>,
    /// `0` is valid (configures identity/matching with nothing ever warm)
    /// but pointless — accepted anyway since it's not actually incorrect.
    pub warm_count: u32,
    /// Max live instances (warm + claimed) at once. Omitted = unbounded
    /// (a claim past warm just cold-creates). When set, a claim at the
    /// ceiling queues (up to `pool_claim::POOL_QUEUE_TIMEOUT`) rather
    /// than rejecting or exceeding it.
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
    /// Snapshots actually sitting warm right now — can be under
    /// `warm_count` right after creation or a claim drain;
    /// `pool_replenisher` tops it back up in the background, not instantly.
    warm_ready: usize,
    /// Live instances right now, tracked regardless of `max_count` (useful
    /// visibility either way) but only enforced against when it's set.
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

/// Removes a pool's configuration and destroys whatever it has warm —
/// `pool_replenisher` naturally stops once it's gone from
/// `AppState::pools`. Wakes anything queued on this pool one last time so
/// a queued caller fails clearly instead of waiting out its own timeout.
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
