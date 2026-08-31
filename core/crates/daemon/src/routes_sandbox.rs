//! Sandbox lifecycle: create, list, stop. Exec and file operations live in
//! `routes_exec` — split out because they share a `call_agent` helper that
//! has nothing to do with lifecycle management. Name-based lookup/
//! get-or-create lives in `routes_sandbox_name` — a distinct enough
//! concern (crosses into snapshot territory, needs the per-name lock)
//! that folding it in here would blow well past this file's existing
//! ~300-line-ish shape for no structural reason.

use crate::error::AppError;
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
use sandkiln_vmm::vm::{DriveConfig, Vm, VmConfig};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Deserialize, Default)]
pub struct CreateSandboxRequest {
    /// Caller-given identity, unique among live sandboxes and held
    /// snapshots at the moment it's claimed (`409` if already taken).
    /// Optional — naming is opt-in. See `Sandbox::name`'s doc comment and
    /// `routes_sandbox_name` for looking a sandbox up by name later.
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) tags: HashMap<String, String>,
    /// Existing persistent drives (see `POST /drives`) to attach at boot,
    /// each becoming its own block device inside the guest.
    #[serde(default)]
    pub(crate) drives: Vec<DriveAttachment>,
    /// Overrides the daemon's configured default vCPU count
    /// (`SANDKILN_VCPU_COUNT`) for this one sandbox. Omitted means "use
    /// the default" — today's behavior, unchanged. Rejected outright
    /// (`400`) rather than clamped if it's `0` or exceeds the configured
    /// ceiling (`SANDKILN_MAX_VCPU_COUNT`) — see `resolve_resource_override`.
    #[serde(default)]
    pub(crate) vcpu_count: Option<u8>,
    /// Overrides the daemon's configured default memory size in MiB
    /// (`SANDKILN_MEM_SIZE_MIB`) for this one sandbox. Same semantics as
    /// `vcpu_count` above, checked against `SANDKILN_MAX_MEM_SIZE_MIB`.
    #[serde(default)]
    pub(crate) mem_size_mib: Option<u32>,
}

#[derive(Serialize)]
pub struct CreateSandboxResponse {
    id: String,
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

    // Held for the rest of this function whenever a name was given —
    // serializes this call against any other concurrent claim of the
    // same name (another named `create_sandbox`, or
    // `routes_sandbox_name::get_or_create_sandbox`) so two callers racing
    // on a brand-new name can't both pass the uniqueness check below and
    // both create a sandbox. See `AppState::lock_name`.
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

/// The actual "boot a sandbox" mechanics, shared by `create_sandbox`
/// (`POST /sandboxes`) and `routes_sandbox_name::get_or_create_sandbox`'s
/// create-fresh path. Does **not** check name uniqueness itself — both
/// callers already did that under `AppState::lock_name` before reaching
/// here, and re-checking would just be redundant work under the same
/// lock they're still holding.
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

    let id = Uuid::new_v4().to_string();
    let rootfs_path = std::env::temp_dir().join(format!("sandkiln-rootfs-{id}.ext4"));
    let attached_drives: Vec<AttachedDrive> =
        request.drives.iter().map(|d| AttachedDrive { drive_id: d.id.clone(), read_only: d.read_only }).collect();
    // Firecracker's own drive_id namespace is per-VM, but prefix these
    // anyway to keep them unambiguously distinct from the reserved
    // "rootfs" id regardless of what a drive's storage id looks like.
    // Firecracker only allows alphanumerics and underscores in a
    // drive_id (drive ids here are UUIDs, which contain hyphens) — '-'
    // has to become '_', not just the prefix's own separator.
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
        move || -> std::io::Result<(Vm, Lease, Option<u32>)> {
            let span = tracing::Span::current();
            // Copying the rootfs and leasing a network are independent —
            // running them concurrently overlaps the (currently dominant)
            // cost of the rootfs copy with the lease instead of paying for
            // both serially.
            let (copy_result, lease_result) = std::thread::scope(|scope| {
                let copy_handle =
                    scope.spawn(|| span.in_scope(|| clone_rootfs(&state.config.base_rootfs_path, &rootfs_path)));
                let lease_handle = scope.spawn(|| span.in_scope(|| state.network.lease()));
                (copy_handle.join().expect("rootfs copy thread panicked"), lease_handle.join().expect("lease thread panicked"))
            });
            copy_result?;
            let lease = lease_result?;

            // A jail id (uid == gid, leased from `AppState::jailer_ids`)
            // is the third resource a sandbox needs, alongside the rootfs
            // copy and the network lease — leased after both since it's
            // an in-memory pool pop (no I/O to overlap with), and
            // released immediately if leasing it is the thing that fails,
            // exactly like a failed `Vm::boot` releases the network lease
            // below.
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
            });
            match vm {
                Ok(vm) => {
                    state.metrics.record_boot_duration_ms(boot_started.elapsed().as_secs_f64() * 1000.0);
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

    let sandbox = Sandbox {
        id: id.clone(),
        vm,
        network: Some(network),
        rootfs_path,
        attached_drives,
        jail_id,
        tags: request.tags,
        created_at: SystemTime::now(),
        last_activity: std::sync::Mutex::new(std::time::Instant::now()),
        source_snapshot_id: None,
        name: request.name,
    };
    state.sandboxes.lock().unwrap().insert(id.clone(), sandbox);
    state.metrics.record_sandbox_created();

    Ok(id)
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

/// Filters by tag by passing `?tag.<key>=<value>` query params — a
/// sandbox must match every one given, if any are given.
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

/// Whether stopping preserves this sandbox's state (the default) or
/// destroys it outright — the query-string form of `DELETE
/// /sandboxes/:id?keep=false`. Pulled out as a pure parser for direct
/// unit testing, mirroring `resolve_resource_override` above.
fn parse_keep(params: &HashMap<String, String>) -> Result<bool, String> {
    match params.get("keep").map(String::as_str) {
        None | Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(other) => Err(format!("invalid 'keep' query parameter '{other}': expected 'true' or 'false'")),
    }
}

#[derive(Serialize)]
pub struct StopSandboxResponse {
    /// Whether this stop actually produced a new `Snapshot` this sandbox
    /// can be resumed from. `false` either because the caller explicitly
    /// asked for full destruction (`?keep=false`) or because this
    /// particular sandbox had nothing new to preserve (a fork — see
    /// `stop_sandbox_by_id`'s doc comment).
    kept: bool,
    snapshot_id: Option<String>,
}

/// `DELETE /sandboxes/:id` — stops a sandbox. As of the "persistent by
/// default" behavior (see `stop_sandbox_by_id`'s doc comment), the
/// default response is `200` with a JSON body reporting what happened,
/// not the old bare `204`: there is now new information worth returning
/// (a snapshot id) that wasn't there when this only ever destroyed. The
/// explicit-destroy path (`?keep=false`) keeps the original `204`
/// contract exactly — nothing new to report, unchanged from before this
/// feature existed.
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

/// What `stop_sandbox_by_id` actually did — used to shape `DELETE`'s
/// response body; the idle reaper (`idle_reaper::run`) only cares whether
/// it succeeded at all.
pub(crate) enum StopOutcome {
    Snapshotted(String),
    Destroyed,
}

/// Every way `stop_sandbox_by_id` can fail. Distinct from `AppError` so
/// callers other than the HTTP route (namely `idle_reaper`) can react to
/// `CannotPreserve` without going through an HTTP-status-shaped type —
/// see `idle_reaper::reap_once`, which falls back to a full destroy on
/// exactly this variant instead of leaking the sandbox forever.
pub(crate) enum StopError {
    NotFound,
    /// `keep=true` was requested (explicitly or by default) but this
    /// particular sandbox structurally can't be snapshotted right now —
    /// see `SnapshotBlocked`. Note a *forked* sandbox never produces this:
    /// `stop_sandbox_by_id` treats that case as a silent, correct destroy
    /// rather than an error (see its doc comment), since a fork has
    /// nothing new to preserve. Only a jailed sandbox reaches here.
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
        // Unreachable via `stop_sandbox_by_id` today (forks are handled
        // as a silent destroy, not this error) — kept exhaustive rather
        // than `unreachable!()` so a future change to that logic fails to
        // compile loudly instead of panicking at runtime if it ever does
        // start reaching here.
        SnapshotBlocked::ForkedFrom(source) => AppError::Conflict(format!(
            "this sandbox was forked from snapshot {source} and can't be independently snapshotted — retry with \
             ?keep=false to destroy it instead"
        )),
    }
}

/// Stops a sandbox. `keep=true` (the default — both `DELETE
/// /sandboxes/:id` with no query param and `idle_reaper`'s automatic
/// stop) is the ROADMAP's "persistent by default" behavior: this
/// internally does what `POST /sandboxes/:id/snapshot` does (pause,
/// snapshot to disk, stop the VM), landing the sandbox as a `Snapshot`
/// record — including its `name`, if it had one — instead of deleting
/// its rootfs and releasing its network lease for good. `keep=false` is
/// the explicit opt-out, for a caller who genuinely wants full
/// destruction with nothing left behind (e.g. a short-lived CI sandbox
/// that will never come back) — it does exactly what stopping a sandbox
/// always used to do.
///
/// A forked sandbox (`source_snapshot_id.is_some()`) is a special case
/// under `keep=true`: it shares its rootfs file with the snapshot it came
/// from rather than owning a private copy, so it structurally can't be
/// snapshotted again on its own (see `SnapshotBlocked::ForkedFrom`) — but
/// that's fine, not an error, because that shared snapshot *already is*
/// this identity's durable state, untouched by the fork's ephemeral VM.
/// There's nothing new to preserve, so `keep=true`'s intent is already
/// satisfied by destroying just the fork (which, per
/// `destroy_sandbox_by_id`'s own doc comment, never touches a fork's
/// shared rootfs/network anyway). A jailed sandbox has no such fallback —
/// Firecracker's jailed snapshot/resume path genuinely isn't supported —
/// so that case surfaces as `StopError::CannotPreserve` instead of
/// silently destroying state a caller's default expectation says should
/// have survived.
///
/// Shared by the `DELETE` route above and the idle reaper
/// (`idle_reaper::run`) — both go through this one path rather than a
/// second, drifted copy of stop logic, and both get the same
/// preserve-by-default behavior for the same reason: consistency between
/// an explicit stop and an automatic idle-timeout stop.
pub(crate) async fn stop_sandbox_by_id(state: Arc<AppState>, id: String, keep: bool) -> Result<StopOutcome, StopError> {
    if keep {
        match snapshot_and_stop(state.clone(), id.clone()).await {
            Ok(snapshot_id) => return Ok(StopOutcome::Snapshotted(snapshot_id)),
            Err(SnapshotStopError::NotFound) => return Err(StopError::NotFound),
            Err(SnapshotStopError::Io(e)) => return Err(StopError::Io(e)),
            Err(SnapshotStopError::Blocked(SnapshotBlocked::ForkedFrom(_))) => {
                // Falls through to the destroy below — see this
                // function's doc comment for why that's correct, not a
                // silent downgrade.
            }
            Err(SnapshotStopError::Blocked(reason @ SnapshotBlocked::Jailed)) => {
                return Err(StopError::CannotPreserve(reason));
            }
        }
    }
    destroy_sandbox_by_id(state, id).await
}

/// Removes a sandbox from the map and tears it down outright: VM stop,
/// network release, rootfs cleanup. The original (pre-naming-feature)
/// "stop a sandbox" behavior — now reached via `keep=false`, or
/// internally when `keep=true` has nothing new to preserve for a forked
/// sandbox (see `stop_sandbox_by_id`'s doc comment).
///
/// A sandbox forked from a snapshot (`source_snapshot_id.is_some()`,
/// see `routes_snapshot::fork_snapshot`) doesn't own its rootfs file or
/// network lease — both still belong to the snapshot, so they're neither
/// deleted nor released here. What it *does* release is the snapshot's
/// fork lock (`Snapshot::forked_into`), letting a later `/fork` or
/// `/resume` proceed — but only after `vm.stop()` returns, which kills
/// and waits on the Firecracker process: clearing the lock any earlier
/// would let a new fork start writing the shared rootfs file before the
/// old one has actually stopped touching it.
async fn destroy_sandbox_by_id(state: Arc<AppState>, id: String) -> Result<StopOutcome, StopError> {
    let sandbox = state.sandboxes.lock().unwrap().remove(&id).ok_or(StopError::NotFound)?;
    let source_snapshot_id = sandbox.source_snapshot_id.clone();
    let owns_rootfs = source_snapshot_id.is_none();

    spawn_blocking_in_current_span("stop task panicked", {
        let state = state.clone();
        move || {
            let _ = sandbox.vm.stop();
            if let Some(network) = sandbox.network {
                let _ = state.network.release(network);
            }
            if owns_rootfs {
                let _ = std::fs::remove_file(&sandbox.rootfs_path);
            }
            // `Vm::stop` already removed this sandbox's chroot directory if
            // it was jailed — this releases the daemon-level uid/gid
            // allocation, a separate resource `Vm` has no visibility into.
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

    Ok(StopOutcome::Destroyed)
}

/// Resolves a per-request resource override (`vcpu_count`/`mem_size_mib`
/// on `CreateSandboxRequest`) against the daemon's configured default and
/// ceiling. `None` (the field omitted) returns `default` unchanged —
/// today's behavior for a caller that doesn't ask for anything special. A
/// caller-supplied `0` (meaningless — a VM can't run with zero vCPUs or
/// zero memory) or anything above `max` is rejected outright rather than
/// silently clamped, so an unreasonable request fails loudly instead of
/// quietly running with less than the caller thought they'd get. A
/// negative value can't reach here at all: `vcpu_count`/`mem_size_mib`
/// deserialize as unsigned integers, so `serde_json` already rejects a
/// negative number in the request body before this is ever called.
fn resolve_resource_override<T>(requested: Option<T>, default: T, max: T, field: &str) -> Result<T, String>
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

/// Returns the first item that's already been seen, if any.
fn first_duplicate<'a>(mut items: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut seen = HashSet::new();
    items.find(|item| !seen.insert(*item))
}

/// Clones the base rootfs for one sandbox. Uses `cp --reflink=auto`
/// rather than `std::fs::copy` so this becomes an instant copy-on-write
/// clone for free on a filesystem that supports it (XFS, Btrfs) — on
/// ext4 (what the dev box runs) `--reflink=auto` just falls back to an
/// ordinary copy, so this has no effect there, but costs nothing either.
fn clone_rootfs(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
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
