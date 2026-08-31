//! Sandbox lifecycle: create, list, stop. Exec and file operations live in
//! `routes_exec` — split out because they share a `call_agent` helper that
//! has nothing to do with lifecycle management.

use crate::error::AppError;
use crate::routes_drives::DriveAttachment;
use crate::sandbox::Sandbox;
use crate::state::{can_attach_read_only, describe_drive_holders, AppState, AttachedDrive};
use crate::tracing_util::spawn_blocking_in_current_span;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
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
    #[serde(default)]
    tags: HashMap<String, String>,
    /// Existing persistent drives (see `POST /drives`) to attach at boot,
    /// each becoming its own block device inside the guest.
    #[serde(default)]
    drives: Vec<DriveAttachment>,
    /// Overrides the daemon's configured default vCPU count
    /// (`SANDKILN_VCPU_COUNT`) for this one sandbox. Omitted means "use
    /// the default" — today's behavior, unchanged. Rejected outright
    /// (`400`) rather than clamped if it's `0` or exceeds the configured
    /// ceiling (`SANDKILN_MAX_VCPU_COUNT`) — see `resolve_resource_override`.
    #[serde(default)]
    vcpu_count: Option<u8>,
    /// Overrides the daemon's configured default memory size in MiB
    /// (`SANDKILN_MEM_SIZE_MIB`) for this one sandbox. Same semantics as
    /// `vcpu_count` above, checked against `SANDKILN_MAX_MEM_SIZE_MIB`.
    #[serde(default)]
    mem_size_mib: Option<u32>,
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
    };
    state.sandboxes.lock().unwrap().insert(id.clone(), sandbox);
    state.metrics.record_sandbox_created();

    Ok(Json(CreateSandboxResponse { id }))
}

#[derive(Serialize)]
pub struct SandboxSummary {
    id: String,
    created_at_unix: u64,
    tags: HashMap<String, String>,
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
        })
        .collect();
    Json(ListSandboxesResponse { sandboxes })
}

#[tracing::instrument(skip(state))]
pub async fn stop_sandbox(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    stop_sandbox_by_id(state, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes a sandbox from the map and tears it down: VM stop, network
/// release, rootfs cleanup. Shared by the `DELETE` route above and the
/// idle reaper (`idle_reaper::run`) — both need the exact same teardown,
/// just triggered differently.
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
pub(crate) async fn stop_sandbox_by_id(state: Arc<AppState>, id: String) -> Result<(), AppError> {
    let sandbox = state.sandboxes.lock().unwrap().remove(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
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

    Ok(())
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
