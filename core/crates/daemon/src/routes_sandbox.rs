//! Sandbox lifecycle: create, list, stop. Exec/file ops live in
//! `routes_exec` (shares `call_agent`, unrelated to lifecycle). Name
//! lookup/get-or-create lives in `routes_sandbox_name` (crosses into
//! snapshot territory, needs the per-name lock).

use crate::error::AppError;
use crate::metrics::CreatePhase;
use crate::pool_claim::{claim_from_pool, resolve_pool_claim, MAX_POOL_CLAIM_ATTEMPTS, PoolClaim, PoolClaimGuard};
use crate::routes_drives::DriveAttachment;
use crate::routes_snapshot::{snapshot_and_stop, SnapshotBlocked, SnapshotStopError};
use crate::sandbox::Sandbox;
use crate::state::{can_attach_read_only, describe_drive_holders, AppState, AttachedDrive};
use crate::tracing_util::spawn_blocking_in_current_span;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use sandkiln_vmm::jailer::JailLaunch;
use sandkiln_vmm::network::Lease;
use sandkiln_vmm::vm::{DriveConfig, RateLimiter, TokenBucket, Vm, VmConfig};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Deserialize, Default)]
pub struct CreateSandboxRequest {
    /// Unique among live sandboxes + held snapshots when claimed (`409`
    /// otherwise). Optional. See `Sandbox::name`, `routes_sandbox_name`.
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) tags: HashMap<String, String>,
    /// Existing persistent drives (`POST /drives`) to attach at boot.
    #[serde(default)]
    pub(crate) drives: Vec<DriveAttachment>,
    /// Overrides `SANDKILN_VCPU_COUNT` for this sandbox. `400` (not
    /// clamped) if `0` or above `SANDKILN_MAX_VCPU_COUNT` — see
    /// `resolve_resource_override`.
    #[serde(default)]
    pub(crate) vcpu_count: Option<u8>,
    /// Overrides `SANDKILN_MEM_SIZE_MIB`; same rules as `vcpu_count`.
    #[serde(default)]
    pub(crate) mem_size_mib: Option<u32>,
    /// Boots from a registered image (`POST /images`) instead of
    /// `SANDKILN_BASE_ROOTFS`. `404` if the id isn't registered.
    #[serde(default)]
    pub(crate) image_id: Option<String>,
    /// Firecracker's own token-bucket I/O rate limiter, applied to the
    /// rootfs drive, every attached drive, and the network interface.
    /// Omitted = unlimited. At least one sub-field must be set and
    /// non-zero, or `400`.
    #[serde(default)]
    pub(crate) rate_limit: Option<RateLimitRequest>,
    /// Outbound network policy — see `sandkiln_vmm::egress`. Omitted =
    /// unrestricted (today's default). Unlike `drives`/`rate_limit`, can
    /// still match a pre-warmed pool: enforced via iptables applied
    /// *after* boot/resume, never baked into Firecracker's VM state.
    #[serde(default)]
    pub(crate) egress: Option<EgressPolicyRequest>,
    /// Baked in for the sandbox's lifetime; base layer under every
    /// `exec`/`exec-stream` call's own `env` (call wins on conflict —
    /// see `routes_exec::resolve_env`). Persists through
    /// snapshot/resume/fork like `tags`.
    #[serde(default)]
    pub(crate) env: HashMap<String, String>,
}

#[derive(Deserialize, Clone, Copy)]
pub struct RateLimitRequest {
    #[serde(default)]
    pub(crate) bandwidth_bytes_per_sec: Option<u64>,
    #[serde(default)]
    pub(crate) ops_per_sec: Option<u64>,
}

#[derive(Deserialize, Clone)]
pub struct EgressPolicyRequest {
    pub(crate) mode: EgressModeRequest,
    /// IPv4 CIDRs. Validated in `resolve_egress_policy`, not here, so a
    /// bad one gets one clear `400` naming it.
    #[serde(default)]
    pub(crate) allow_cidrs: Vec<String>,
    #[serde(default)]
    pub(crate) deny_cidrs: Vec<String>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum EgressModeRequest {
    AllowAll,
    DenyAll,
}

#[derive(Serialize)]
pub struct CreateSandboxResponse {
    id: String,
}

/// RAII for `AppState::reserve_pending_image_boot`/
/// `release_pending_image_boot` — releases on drop, covering every exit
/// path (happy path, early `?`, panic unwind). No-op when `image_id` is
/// `None`.
struct PendingImageBootGuard {
    state: Arc<AppState>,
    image_id: Option<String>,
}

impl Drop for PendingImageBootGuard {
    fn drop(&mut self) {
        if let Some(image_id) = &self.image_id {
            self.state.release_pending_image_boot(image_id);
        }
    }
}

#[tracing::instrument(skip(state, body))]
pub async fn create_sandbox(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Json<CreateSandboxResponse>, AppError> {
    let request: CreateSandboxRequest = if body.is_empty() {
        CreateSandboxRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?
    };

    // Serializes this call against any other concurrent claim of the
    // same name so two racing callers can't both pass the uniqueness
    // check below. See `AppState::lock_name`.
    let _name_guard = match &request.name {
        Some(name) => {
            crate::routes_sandbox_name::validate_name(name).map_err(AppError::BadRequest)?;
            let guard = state.lock_name(name).await;
            if let Some(holder) = state.name_holder(name) {
                return Err(AppError::Conflict(format!("name '{name}' is already used by {holder}")));
            }
            Some(guard)
        }
        None => None,
    };

    let id = create_sandbox_core(&state, request).await?;
    Ok(Json(CreateSandboxResponse { id }))
}

/// Shared boot mechanics for `create_sandbox` and
/// `routes_sandbox_name::get_or_create_sandbox`'s create-fresh path.
/// Doesn't check name uniqueness — both callers already did, under the
/// same `lock_name` guard still held here.
pub(crate) async fn create_sandbox_core(state: &Arc<AppState>, request: CreateSandboxRequest) -> Result<String, AppError> {
    if let Some(dup) = first_duplicate(request.drives.iter().map(|d| d.id.as_str())) {
        return Err(AppError::BadRequest(format!("drive listed more than once: {dup}")));
    }
    for drive in &request.drives {
        if !state.drives.exists(&drive.id) {
            return Err(AppError::DriveNotFound(drive.id.clone()));
        }
    }
    for drive in &request.drives {
        let holders = state.drive_holders(&drive.id);
        let existing_read_only: Vec<bool> = holders.iter().map(|h| h.read_only).collect();
        if !can_attach_read_only(&existing_read_only, drive.read_only) {
            return Err(AppError::Conflict(format!(
                "drive {} is already attached to {} — a read-write attachment needs exclusive access; \
                 only simultaneous read-only attachments are allowed",
                drive.id,
                describe_drive_holders(&holders)
            )));
        }
    }

    let vcpu_count = resolve_resource_override(request.vcpu_count, state.config.vcpu_count, state.config.max_vcpu_count, "vcpu_count")
        .map_err(AppError::BadRequest)?;
    let mem_size_mib =
        resolve_resource_override(request.mem_size_mib, state.config.mem_size_mib, state.config.max_mem_size_mib, "mem_size_mib")
            .map_err(AppError::BadRequest)?;
    let rate_limit = resolve_rate_limit(&request.rate_limit).map_err(AppError::BadRequest)?;
    let egress = resolve_egress_policy(&request.egress).map_err(AppError::BadRequest)?;

    // `drives`/`rate_limit` are baked into a VM at boot time; a warm
    // snapshot was booted with neither, so either present here means
    // this can never match a pool (see `crate::pool`) — falls through
    // to cold-create and never queues either, for the same reason.
    if request.drives.is_empty() && request.rate_limit.is_none() {
        let key = crate::pool::PoolKey { image_id: request.image_id.clone(), vcpu_count, mem_size_mib };
        // Bounded retry: a warm claim failing its post-resume health
        // check (measured up to ~2-in-3 resumes — not rare) releases its
        // slot via `PoolClaimGuard`'s drop, and re-resolving immediately
        // is what keeps the fallback cold-create still counted against
        // `max_count` instead of silently exceeding it. Bounded only as
        // a safety margin against a pathological run of bad warm
        // snapshots — see `MAX_POOL_CLAIM_ATTEMPTS`.
        for _ in 0..MAX_POOL_CLAIM_ATTEMPTS {
            match resolve_pool_claim(state, &key).await? {
                PoolClaim::Warm { pool_id, snapshot_id } => {
                    let guard = PoolClaimGuard::new(state.clone(), Some(pool_id.clone()));
                    match claim_from_pool(state, snapshot_id, pool_id, &request, egress.clone()).await {
                        Ok(id) => {
                            guard.commit();
                            return Ok(id);
                        }
                        Err(e) => {
                            // `guard` drops uncommitted here, freeing the
                            // slot for the next loop iteration.
                            tracing::warn!(error = %e, "pool claim failed — retrying against the pool once more before falling back unattributed");
                        }
                    }
                }
                PoolClaim::ColdSlot { pool_id } => {
                    // Room under max_count, nothing warm ready — cold-create
                    // below, attributed to this pool.
                    return create_sandbox_cold(state, request, Some(pool_id), vcpu_count, mem_size_mib, rate_limit, egress).await;
                }
                PoolClaim::NoPool => break, // no pool configured for this profile.
            }
        }
    }

    create_sandbox_cold(state, request, None, vcpu_count, mem_size_mib, rate_limit, egress).await
}

/// Cold-boot mechanics. `pool_id` is `Some` when `create_sandbox_core`
/// reserved a `max_count` slot (`PoolClaim::ColdSlot`) — released on
/// failure via `PoolClaimGuard`, committed to `Sandbox::source_pool_id`
/// on success.
async fn create_sandbox_cold(
    state: &Arc<AppState>,
    request: CreateSandboxRequest,
    pool_id: Option<String>,
    vcpu_count: u8,
    mem_size_mib: u32,
    rate_limit: Option<RateLimiter>,
    egress: Option<sandkiln_vmm::egress::EgressPolicy>,
) -> Result<String, AppError> {
    let create_started = Instant::now();
    let pool_guard = PoolClaimGuard::new(state.clone(), pool_id.clone());

    // Reserved before the (slow) boot starts, closing the window where a
    // concurrent `DELETE /images/:id` could remove the file this boot's
    // rootfs copy is reading. `_image_boot_guard` releases it on every
    // exit path.
    if let Some(image_id) = &request.image_id {
        if !state.images.exists(image_id) {
            return Err(AppError::ImageNotFound(image_id.clone()));
        }
        state.reserve_pending_image_boot(image_id);
    }
    let _image_boot_guard = PendingImageBootGuard { state: state.clone(), image_id: request.image_id.clone() };
    let base_rootfs_source = match &request.image_id {
        Some(image_id) => state.images.path_for(image_id),
        None => state.config.base_rootfs_path.clone(),
    };

    let id = Uuid::new_v4().to_string();
    // Guest-reachable via Firecracker's own MMDS (see VmConfig::metadata).
    let metadata = serde_json::json!({ "id": id, "name": &request.name, "tags": &request.tags });
    let rootfs_path = std::env::temp_dir().join(format!("sandkiln-rootfs-{id}.ext4"));
    let attached_drives: Vec<AttachedDrive> =
        request.drives.iter().map(|d| AttachedDrive { drive_id: d.id.clone(), read_only: d.read_only }).collect();
    // Prefixed to stay distinct from the reserved "rootfs" id. Firecracker
    // only allows alphanumerics/underscores in a drive_id, so '-' (these
    // are UUIDs) becomes '_'.
    let extra_drives: Vec<DriveConfig> = request
        .drives
        .iter()
        .map(|d| DriveConfig {
            drive_id: format!("drive_{}", d.id.replace('-', "_")),
            path_on_host: state.drives.path_for(&d.id),
            read_only: d.read_only,
        })
        .collect();

    let (vm, network, jail_id) = spawn_blocking_in_current_span("boot task panicked", {
        let state = state.clone();
        let rootfs_path = rootfs_path.clone();
        let base_rootfs_source = base_rootfs_source.clone();
        let egress = egress.clone();
        move || -> std::io::Result<(Vm, Lease, Option<u32>)> {
            let span = tracing::Span::current();
            // Rootfs copy and network lease run concurrently. The clone
            // dominates (~124ms vs ~4ms on this dev box's ext4), so the
            // concurrency mostly hides nothing today — it stays because
            // it's free and pays off once `cp --reflink=auto` can
            // actually reflink (see `clone_rootfs`). Note the
            // destination is `std::env::temp_dir()`: reflink silently
            // degrades to a full copy across filesystems, the leading
            // (unverified) explanation for why an XFS experiment showed
            // no end-to-end win — see ROADMAP.md's Benchmarking section.
            let setup_started = Instant::now();
            let ((copy_result, copy_elapsed), (lease_result, lease_elapsed)) = std::thread::scope(|scope| {
                let copy_handle = scope.spawn(|| span.in_scope(|| timed(|| clone_rootfs(&base_rootfs_source, &rootfs_path))));
                let lease_handle = scope.spawn(|| span.in_scope(|| timed(|| state.network.lease())));
                (copy_handle.join().expect("rootfs copy thread panicked"), lease_handle.join().expect("lease thread panicked"))
            });
            let setup_elapsed = setup_started.elapsed();
            // Recorded before the `?`s so a create that fails mid-phase
            // still contributes the timing it produced.
            state.metrics.record_create_phase_ms(CreatePhase::RootfsClone, ms(copy_elapsed));
            state.metrics.record_create_phase_ms(CreatePhase::NetworkLease, ms(lease_elapsed));
            state.metrics.record_create_phase_ms(CreatePhase::Setup, ms(setup_elapsed));
            tracing::debug!(
                rootfs_clone_us = copy_elapsed.as_micros(),
                network_lease_us = lease_elapsed.as_micros(),
                setup_join_us = setup_elapsed.as_micros(),
                "cold create setup phases"
            );
            copy_result?;
            let lease = lease_result?;

            // Third resource besides rootfs/lease: an in-memory pool pop,
            // leased last (nothing to overlap it with), released
            // immediately on failure just like a failed `Vm::boot`
            // releases the network lease below.
            let jail_id = match &state.jailer_ids {
                Some(pool) => match pool.lease() {
                    Ok(id) => Some(id),
                    Err(e) => {
                        let _ = state.network.release(lease);
                        return Err(e);
                    }
                },
                None => None,
            };
            let jail = jail_id.and_then(|id| {
                state.config.jailer.as_ref().map(|j| JailLaunch {
                    jailer_bin: j.jailer_bin.clone(),
                    chroot_base_dir: j.chroot_base_dir.clone(),
                    uid: id,
                    gid: id,
                })
            });

            let boot_started = Instant::now();
            let vm = Vm::boot(&VmConfig {
                firecracker_bin: state.config.firecracker_bin.clone(),
                kernel_path: state.config.kernel_path.clone(),
                rootfs_path,
                vcpu_count,
                mem_size_mib,
                network: Some(lease.config.clone()),
                extra_drives,
                jail,
                rate_limit,
                metadata: Some(metadata),
            });
            match vm {
                Ok(vm) => {
                    state.metrics.record_boot_duration_ms(ms(boot_started.elapsed()));
                    // Treated like a failed `Vm::boot`: a policy that
                    // fails to apply must not leave the sandbox silently
                    // unrestricted, so tear down and return the error.
                    if let Some(policy) = &egress {
                        let egress_started = Instant::now();
                        let egress_result =
                            sandkiln_vmm::egress::apply(lease.config.guest_ip, &lease.config.tap_device, state.network.uplink(), policy);
                        state.metrics.record_create_phase_ms(CreatePhase::EgressApply, ms(egress_started.elapsed()));
                        if let Err(e) = egress_result {
                            let _ = vm.stop();
                            let _ = state.network.release(lease);
                            if let (Some(id), Some(pool)) = (jail_id, &state.jailer_ids) {
                                pool.release(id);
                            }
                            return Err(e);
                        }
                    }
                    Ok((vm, lease, jail_id))
                }
                Err(e) => {
                    let _ = state.network.release(lease);
                    if let (Some(id), Some(pool)) = (jail_id, &state.jailer_ids) {
                        pool.release(id);
                    }
                    Err(e)
                }
            }
        }
    })
    .await?;
    let boot_task_elapsed = create_started.elapsed();

    let created_at = SystemTime::now();
    // Best-effort: the VM already booted, so a history-write failure
    // shouldn't fail the whole request — only warn.
    let before_history = Instant::now();
    if let Err(e) = state.history.record_created(&id, request.name.as_deref(), &request.tags, request.image_id.as_deref(), created_at) {
        tracing::warn!(error = %e, sandbox_id = %id, "failed to record sandbox creation in history store");
    }
    let history_elapsed = before_history.elapsed();

    let sandbox = Sandbox {
        id: id.clone(),
        vm,
        network: Some(network),
        rootfs_path,
        attached_drives,
        image_id: request.image_id,
        jail_id,
        tags: request.tags,
        created_at,
        last_activity: std::sync::Mutex::new(std::time::Instant::now()),
        source_snapshot_id: None,
        name: request.name,
        pty_session_count: Default::default(),
        source_pool_id: pool_id,
        egress,
        env: request.env,
        // A cold create (fresh or a pool's own warm boot) is always a
        // lineage root.
        parent_snapshot_id: None,
        mounts: Vec::new(),
        log_sessions: Default::default(),
    };
    state.sandboxes.lock().unwrap().insert(id.clone(), sandbox);
    state.metrics.record_sandbox_created();
    // The reserved slot is now durably owned by the live `Sandbox` via
    // `source_pool_id` — released later by destroy/snapshot-and-stop,
    // not this guard.
    pool_guard.commit();

    let total = create_started.elapsed();
    state.metrics.record_create_phase_ms(CreatePhase::Total, ms(total));
    // `boot_task_us` covers the whole spawn_blocking closure; the gap to
    // `total` is the daemon-side tail (history write, map lock) plus
    // blocking-pool wait time.
    tracing::debug!(
        sandbox_id = %id,
        boot_task_us = boot_task_elapsed.as_micros(),
        history_write_us = history_elapsed.as_micros(),
        total_us = total.as_micros(),
        "cold create complete"
    );

    Ok(id)
}

/// Runs `f`, returning its value plus elapsed time — lets each
/// concurrently-spawned setup phase time itself, since a timer around
/// the join alone can't tell the two apart.
fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let value = f();
    (value, started.elapsed())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[derive(Serialize)]
pub struct SandboxSummary {
    id: String,
    created_at_unix: u64,
    tags: HashMap<String, String>,
    name: Option<String>,
}

#[derive(Serialize)]
pub struct ListSandboxesResponse {
    sandboxes: Vec<SandboxSummary>,
}

/// `?tag.<key>=<value>` query params filter; a sandbox must match all given.
pub async fn list_sandboxes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<ListSandboxesResponse> {
    let tag_filters: Vec<(&str, &str)> = query
        .iter()
        .filter_map(|(k, v)| k.strip_prefix("tag.").map(|key| (key, v.as_str())))
        .collect();

    let sandboxes = state
        .sandboxes
        .lock()
        .unwrap()
        .values()
        .filter(|s| tag_filters.iter().all(|(k, v)| s.tags.get(*k).map(String::as_str) == Some(v)))
        .map(|s| SandboxSummary {
            id: s.id.clone(),
            created_at_unix: s.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            tags: s.tags.clone(),
            name: s.name.clone(),
        })
        .collect();
    Json(ListSandboxesResponse { sandboxes })
}

#[derive(Serialize)]
pub struct HistoryRecordBody {
    id: String,
    name: Option<String>,
    tags: HashMap<String, String>,
    image_id: Option<String>,
    created_at_unix: u64,
    ended_at_unix: Option<u64>,
    end_reason: Option<String>,
    final_snapshot_id: Option<String>,
}

impl From<sandkiln_store::HistoryRecord> for HistoryRecordBody {
    fn from(r: sandkiln_store::HistoryRecord) -> Self {
        Self {
            id: r.id,
            name: r.name,
            tags: r.tags,
            image_id: r.image_id,
            created_at_unix: r.created_at_unix,
            ended_at_unix: r.ended_at_unix,
            end_reason: r.end_reason,
            final_snapshot_id: r.final_snapshot_id,
        }
    }
}

#[derive(Serialize)]
pub struct SandboxHistoryResponse {
    history: Vec<HistoryRecordBody>,
}

/// Durable history, independent of the live `sandboxes` map — survives a
/// restart, unlike `GET /sandboxes` (see `sandkiln-store`'s doc comment:
/// it cannot revive a stopped sandbox). `?live_only=`/`?limit=` (default
/// `sandkiln_store::DEFAULT_LIST_LIMIT`). Newest-created first.
pub async fn sandbox_history(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<SandboxHistoryResponse>, AppError> {
    let live_only = query.get("live_only").map(|v| v == "true");
    let limit = query.get("limit").and_then(|v| v.parse::<u32>().ok());
    let filter = sandkiln_store::HistoryFilter { live_only, limit };

    let records = spawn_blocking_in_current_span("history list task panicked", {
        let state = state.clone();
        move || state.history.list(&filter)
    })
    .await
    .map_err(|e| AppError::Internal(std::io::Error::other(e.to_string())))?;

    Ok(Json(SandboxHistoryResponse { history: records.into_iter().map(HistoryRecordBody::from).collect() }))
}

/// `DELETE /sandboxes/:id?keep=false` query-string parse. Pure, for unit
/// testing (mirrors `resolve_resource_override`).
fn parse_keep(params: &HashMap<String, String>) -> Result<bool, String> {
    match params.get("keep").map(String::as_str) {
        None | Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(other) => Err(format!("invalid 'keep' query parameter '{other}': expected 'true' or 'false'")),
    }
}

#[derive(Serialize)]
pub struct StopSandboxResponse {
    /// Whether this stop produced a new resumable `Snapshot`. `false`
    /// either from `?keep=false` or because this sandbox had nothing new
    /// to preserve (a fork — see `stop_sandbox_by_id`).
    kept: bool,
    snapshot_id: Option<String>,
}

/// `DELETE /sandboxes/:id`. Default response is `200` with a JSON body
/// (there's now a snapshot id worth returning, per "persistent by
/// default" — see `stop_sandbox_by_id`); `?keep=false` keeps the
/// original bare `204` contract.
#[tracing::instrument(skip(state))]
pub async fn stop_sandbox(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<axum::response::Response, AppError> {
    let keep = parse_keep(&params).map_err(AppError::BadRequest)?;

    let outcome = stop_sandbox_by_id(state, id.clone(), keep).await.map_err(|e| match e {
        StopError::NotFound => AppError::NotFound(id),
        StopError::CannotPreserve(reason) => cannot_preserve_error(reason),
        StopError::Io(e) => AppError::from(e),
    })?;

    if !keep {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let (kept, snapshot_id) = match outcome {
        StopOutcome::Snapshotted(snapshot_id) => (true, Some(snapshot_id)),
        StopOutcome::Destroyed => (false, None),
    };
    Ok(Json(StopSandboxResponse { kept, snapshot_id }).into_response())
}

/// What `stop_sandbox_by_id` did — shapes `DELETE`'s response;
/// `idle_reaper::run` only cares whether it succeeded.
pub(crate) enum StopOutcome {
    Snapshotted(String),
    Destroyed,
}

/// Every way `stop_sandbox_by_id` can fail. Separate from `AppError` so
/// `idle_reaper` can react to `CannotPreserve` (fall back to a full
/// destroy) without an HTTP-shaped type.
pub(crate) enum StopError {
    NotFound,
    /// `keep=true` requested but this sandbox structurally can't be
    /// snapshotted right now (see `SnapshotBlocked`). A forked sandbox
    /// never reaches here — that's a silent, correct destroy instead
    /// (nothing new to preserve); only a jailed sandbox does.
    CannotPreserve(SnapshotBlocked),
    Io(std::io::Error),
}

fn cannot_preserve_error(reason: SnapshotBlocked) -> AppError {
    match reason {
        SnapshotBlocked::Jailed => AppError::Conflict(
            "this sandbox is jailed, and snapshotting a jailed sandbox is not supported yet — it can't be \
             stopped-and-preserved by default; retry with ?keep=false to destroy it instead"
                .to_string(),
        ),
        // Unreachable via `stop_sandbox_by_id` today (forks are a silent
        // destroy, not this error) — kept exhaustive so a future change
        // fails to compile instead of panicking if it ever does reach here.
        SnapshotBlocked::ForkedFrom(source) => AppError::Conflict(format!(
            "this sandbox was forked from snapshot {source} and can't be independently snapshotted — retry with \
             ?keep=false to destroy it instead"
        )),
    }
}

/// `keep=true` (the default, for both `DELETE` and `idle_reaper`) is
/// "persistent by default": pause, snapshot to disk, stop — landing a
/// `Snapshot` record (with `name`, if any) instead of deleting rootfs
/// and releasing the lease. `keep=false` is the full-destruction opt-out
/// (e.g. a short-lived CI sandbox that won't come back).
///
/// A forked sandbox under `keep=true` is a special case: it shares its
/// source snapshot's rootfs rather than owning a copy, so it can't be
/// snapshotted again (`SnapshotBlocked::ForkedFrom`) — but that's fine,
/// not an error, since that shared snapshot already *is* this identity's
/// durable state. Destroying just the fork (which never touches the
/// shared rootfs/network — see `destroy_sandbox_by_id`) already
/// satisfies `keep=true`'s intent. A jailed sandbox has no such
/// fallback (jailed snapshot/resume genuinely isn't supported), so that
/// surfaces as `StopError::CannotPreserve` instead of silently
/// destroying state the caller expected to survive.
///
/// Shared by the `DELETE` route and `idle_reaper::run` so an explicit
/// stop and an automatic idle-timeout stop behave identically.
pub(crate) async fn stop_sandbox_by_id(state: Arc<AppState>, id: String, keep: bool) -> Result<StopOutcome, StopError> {
    if keep {
        match snapshot_and_stop(state.clone(), id.clone()).await {
            Ok(snapshot_id) => return Ok(StopOutcome::Snapshotted(snapshot_id)),
            Err(SnapshotStopError::NotFound) => return Err(StopError::NotFound),
            Err(SnapshotStopError::Io(e)) => return Err(StopError::Io(e)),
            Err(SnapshotStopError::Blocked(SnapshotBlocked::ForkedFrom(_))) => {
                // Falls through to destroy below — correct, not a silent
                // downgrade (see doc comment above).
            }
            Err(SnapshotStopError::Blocked(reason @ SnapshotBlocked::Jailed)) => {
                return Err(StopError::CannotPreserve(reason));
            }
        }
    }
    destroy_sandbox_by_id(state, id).await
}

/// Full teardown: VM stop, network release, rootfs cleanup. Reached via
/// `keep=false`, or internally when `keep=true` has nothing new to
/// preserve for a fork.
///
/// A forked sandbox (`source_snapshot_id.is_some()`) doesn't own its
/// rootfs or lease — both still belong to the snapshot, so neither is
/// touched here. What it does release is the snapshot's fork lock
/// (`Snapshot::forked_into`), unblocking a later `/fork`/`/resume` — but
/// only after `vm.stop()` returns (kills and waits on the process), so a
/// new fork can't start writing the shared rootfs before the old one has
/// actually stopped touching it.
async fn destroy_sandbox_by_id(state: Arc<AppState>, id: String) -> Result<StopOutcome, StopError> {
    let sandbox = state.sandboxes.lock().unwrap().remove(&id).ok_or(StopError::NotFound)?;
    let source_snapshot_id = sandbox.source_snapshot_id.clone();

    // Pool membership ends here — see the identical release in
    // `snapshot_and_stop` for why it doesn't carry onto anything
    // resumed/forked later.
    if let Some(pool_id) = &sandbox.source_pool_id {
        if let Some(pool) = state.pools.lock().unwrap().get_mut(pool_id) {
            pool.record_release();
        }
    }

    spawn_blocking_in_current_span("stop task panicked", {
        let state = state.clone();
        move || {
            let _ = sandbox.vm.stop();
            if let Some(network) = sandbox.network {
                // This sandbox's egress chain (if any) goes before the
                // lease is released, matching "removed only when the
                // lease is released." Safe even if no policy was ever
                // applied — see `egress::remove`.
                sandkiln_vmm::egress::remove(network.config.guest_ip, &network.config.tap_device, state.network.uplink());
                let _ = state.network.release(network);
            }
            // Every `Sandbox` owns a private rootfs outright (a fork
            // gets its own clone at fork time — see
            // `fork_snapshot`), so nothing else is left dangling by
            // removing this file.
            let _ = std::fs::remove_file(&sandbox.rootfs_path);
            // `Vm::stop` already removed the chroot dir if jailed; this
            // releases the separate daemon-level uid/gid allocation.
            if let (Some(id), Some(pool)) = (sandbox.jail_id, &state.jailer_ids) {
                pool.release(id);
            }
        }
    })
    .await;

    if let Some(snapshot_id) = source_snapshot_id {
        if let Some(snapshot) = state.snapshots.lock().unwrap().get_mut(&snapshot_id) {
            snapshot.forked_into = None;
        }
    }

    if let Err(e) = state.history.record_ended(&id, SystemTime::now(), sandkiln_store::EndReason::Destroyed, None) {
        tracing::warn!(error = %e, sandbox_id = %id, "failed to record sandbox destruction in history store");
    }

    Ok(StopOutcome::Destroyed)
}

/// Resolves a per-request override against the configured default/
/// ceiling. `None` → `default`. `0` or above `max` is rejected (`400`),
/// not clamped — a bad request fails loudly instead of silently running
/// with less than expected. A negative value can't reach here at all:
/// these fields deserialize as unsigned, so `serde_json` rejects it first.
pub(crate) fn resolve_resource_override<T>(requested: Option<T>, default: T, max: T, field: &str) -> Result<T, String>
where
    T: PartialOrd + Copy + Default + std::fmt::Display,
{
    match requested {
        None => Ok(default),
        Some(value) if value == T::default() => Err(format!("{field} must be greater than 0")),
        Some(value) if value > max => Err(format!("{field} {value} exceeds the configured maximum of {max}")),
        Some(value) => Ok(value),
    }
}

/// Resolves a requested `rate_limit` into the `RateLimiter` Firecracker
/// understands. `None` → unlimited. Neither sub-field set, or either
/// `0`, is rejected (meaningless, not treated as unlimited) — same
/// convention as `resolve_resource_override`. Each bucket refills to
/// `size` once per second (`refill_time: 1000`), no initial burst — the
/// simplest bytes/ops-per-second mapping; burst tuning isn't exposed yet.
fn resolve_rate_limit(requested: &Option<RateLimitRequest>) -> Result<Option<RateLimiter>, String> {
    let Some(req) = requested else { return Ok(None) };
    if req.bandwidth_bytes_per_sec.is_none() && req.ops_per_sec.is_none() {
        return Err("rate_limit must set at least one of bandwidth_bytes_per_sec/ops_per_sec".to_string());
    }
    let to_bucket = |field: &str, value: Option<u64>| -> Result<Option<TokenBucket>, String> {
        match value {
            None => Ok(None),
            Some(0) => Err(format!("rate_limit.{field} must be greater than 0")),
            Some(size) => Ok(Some(TokenBucket { size, one_time_burst: None, refill_time: 1000 })),
        }
    };
    Ok(Some(RateLimiter {
        bandwidth: to_bucket("bandwidth_bytes_per_sec", req.bandwidth_bytes_per_sec)?,
        ops: to_bucket("ops_per_sec", req.ops_per_sec)?,
    }))
}

/// Validates every CIDR up front — one clear `400` naming the bad entry,
/// rather than a cryptic iptables failure later inside a boot task.
fn resolve_egress_policy(requested: &Option<EgressPolicyRequest>) -> Result<Option<sandkiln_vmm::egress::EgressPolicy>, String> {
    let Some(req) = requested else { return Ok(None) };
    for cidr in req.allow_cidrs.iter().chain(&req.deny_cidrs) {
        sandkiln_vmm::egress::validate_cidr(cidr)?;
    }
    let mode = match req.mode {
        EgressModeRequest::AllowAll => sandkiln_vmm::egress::EgressMode::AllowAll,
        EgressModeRequest::DenyAll => sandkiln_vmm::egress::EgressMode::DenyAll,
    };
    Ok(Some(sandkiln_vmm::egress::EgressPolicy { mode, allow_cidrs: req.allow_cidrs.clone(), deny_cidrs: req.deny_cidrs.clone() }))
}

/// First item already seen, if any.
fn first_duplicate<'a>(mut items: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut seen = HashSet::new();
    items.find(|item| !seen.insert(*item))
}

/// `cp --reflink=auto` instead of `std::fs::copy`: an instant CoW clone
/// on a filesystem that supports it (XFS, Btrfs); on ext4 (this dev box)
/// it's just an ordinary copy. `pub(crate)`: also used by
/// `routes_snapshot`'s history-retaining resume and
/// `routes_snapshot_history`'s restore, for the same reason — handing a
/// *shared* rootfs to a new mutating sandbox would corrupt whatever else
/// depends on that file staying unchanged.
pub(crate) fn clone_rootfs(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cp").arg("--reflink=auto").arg(src).arg(dst).status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!("cp --reflink=auto {src:?} {dst:?} failed: {status}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keep_defaults_to_true_when_absent() {
        assert_eq!(parse_keep(&HashMap::new()), Ok(true));
    }

    #[test]
    fn parse_keep_accepts_explicit_true_and_false() {
        assert_eq!(parse_keep(&HashMap::from([("keep".to_string(), "true".to_string())])), Ok(true));
        assert_eq!(parse_keep(&HashMap::from([("keep".to_string(), "false".to_string())])), Ok(false));
    }

    #[test]
    fn parse_keep_rejects_anything_else() {
        let err = parse_keep(&HashMap::from([("keep".to_string(), "yes".to_string())])).unwrap_err();
        assert!(err.contains("yes"), "message was: {err}");
    }

    #[test]
    fn cannot_preserve_error_for_jailed_mentions_the_opt_out() {
        let AppError::Conflict(message) = cannot_preserve_error(SnapshotBlocked::Jailed) else {
            panic!("expected Conflict")
        };
        assert!(message.contains("keep=false"), "message was: {message}");
    }

    #[test]
    fn resolve_resource_override_uses_the_default_when_omitted() {
        assert_eq!(resolve_resource_override(None, 2u8, 16u8, "vcpu_count"), Ok(2));
    }

    #[test]
    fn resolve_resource_override_accepts_a_value_within_the_ceiling() {
        assert_eq!(resolve_resource_override(Some(8u8), 2u8, 16u8, "vcpu_count"), Ok(8));
    }

    #[test]
    fn resolve_resource_override_accepts_a_value_exactly_at_the_ceiling() {
        assert_eq!(resolve_resource_override(Some(16u8), 2u8, 16u8, "vcpu_count"), Ok(16));
    }

    #[test]
    fn resolve_resource_override_rejects_zero() {
        assert_eq!(resolve_resource_override(Some(0u8), 2u8, 16u8, "vcpu_count"), Err("vcpu_count must be greater than 0".to_string()));
    }

    #[test]
    fn resolve_resource_override_rejects_above_the_ceiling() {
        assert_eq!(
            resolve_resource_override(Some(17u8), 2u8, 16u8, "vcpu_count"),
            Err("vcpu_count 17 exceeds the configured maximum of 16".to_string())
        );
    }

    #[test]
    fn resolve_resource_override_works_for_mem_size_mib_too() {
        assert_eq!(resolve_resource_override(Some(4096u32), 512u32, 16384u32, "mem_size_mib"), Ok(4096));
        assert_eq!(
            resolve_resource_override(Some(u32::MAX), 512u32, 16384u32, "mem_size_mib"),
            Err(format!("mem_size_mib {} exceeds the configured maximum of 16384", u32::MAX))
        );
    }

    #[test]
    fn first_duplicate_finds_a_repeat() {
        assert_eq!(first_duplicate(["a", "b", "a"].into_iter()), Some("a"));
        assert_eq!(first_duplicate(["a", "b", "c", "b"].into_iter()), Some("b"));
    }

    #[test]
    fn first_duplicate_none_when_all_unique() {
        assert_eq!(first_duplicate(["a", "b", "c"].into_iter()), None);
        assert_eq!(first_duplicate(std::iter::empty()), None);
    }

    #[test]
    fn resolve_egress_policy_returns_none_when_omitted() {
        assert_eq!(resolve_egress_policy(&None), Ok(None));
    }

    #[test]
    fn resolve_egress_policy_accepts_well_formed_cidrs_in_both_lists() {
        let requested = Some(EgressPolicyRequest {
            mode: EgressModeRequest::DenyAll,
            allow_cidrs: vec!["10.0.0.0/8".to_string()],
            deny_cidrs: vec!["192.168.1.1/32".to_string()],
        });
        let policy = resolve_egress_policy(&requested).unwrap().unwrap();
        assert_eq!(policy.mode, sandkiln_vmm::egress::EgressMode::DenyAll);
        assert_eq!(policy.allow_cidrs, vec!["10.0.0.0/8".to_string()]);
        assert_eq!(policy.deny_cidrs, vec!["192.168.1.1/32".to_string()]);
    }

    #[test]
    fn resolve_egress_policy_rejects_a_malformed_allow_cidr() {
        let requested = Some(EgressPolicyRequest {
            mode: EgressModeRequest::AllowAll,
            allow_cidrs: vec!["not-a-cidr".to_string()],
            deny_cidrs: vec![],
        });
        let err = resolve_egress_policy(&requested).unwrap_err();
        assert!(err.contains("not-a-cidr"), "message was: {err}");
    }

    #[test]
    fn resolve_egress_policy_rejects_a_malformed_deny_cidr() {
        let requested = Some(EgressPolicyRequest {
            mode: EgressModeRequest::AllowAll,
            allow_cidrs: vec![],
            deny_cidrs: vec!["10.0.0.0/33".to_string()],
        });
        let err = resolve_egress_policy(&requested).unwrap_err();
        assert!(err.contains("10.0.0.0/33"), "message was: {err}");
    }

    #[test]
    fn clone_rootfs_copies_real_file_contents() {
        let dir = std::env::temp_dir().join(format!("sandkiln-clone-rootfs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        std::fs::write(&src, b"some rootfs bytes").unwrap();

        clone_rootfs(&src, &dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"some rootfs bytes");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clone_rootfs_fails_cleanly_for_a_missing_source() {
        let dir = std::env::temp_dir().join(format!("sandkiln-clone-rootfs-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let result = clone_rootfs(&dir.join("does-not-exist.bin"), &dir.join("dst.bin"));
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
