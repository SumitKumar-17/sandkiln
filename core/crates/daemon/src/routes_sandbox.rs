//! Sandbox lifecycle: create, list, stop. Exec and file operations live in
//! `routes_exec` — split out because they share a `call_agent` helper that
//! has nothing to do with lifecycle management.

use crate::error::AppError;
use crate::routes_drives::DriveAttachment;
use crate::sandbox::Sandbox;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use sandkiln_vmm::network::Lease;
use sandkiln_vmm::vm::{DriveConfig, Vm, VmConfig};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Deserialize, Default)]
pub struct CreateSandboxRequest {
    #[serde(default)]
    tags: HashMap<String, String>,
    /// Existing persistent drives (see `POST /drives`) to attach at boot,
    /// each becoming its own block device inside the guest.
    #[serde(default)]
    drives: Vec<DriveAttachment>,
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
        if let Some(holder) = state.drive_holder(&drive.id) {
            return Err(AppError::Conflict(format!("drive {} is already attached to {holder}", drive.id)));
        }
    }

    let id = Uuid::new_v4().to_string();
    let rootfs_path = std::env::temp_dir().join(format!("sandkiln-rootfs-{id}.ext4"));
    let attached_drive_ids: Vec<String> = request.drives.iter().map(|d| d.id.clone()).collect();
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

    let (vm, network) = tokio::task::spawn_blocking({
        let state = state.clone();
        let rootfs_path = rootfs_path.clone();
        move || -> std::io::Result<(Vm, Lease)> {
            // Copying the rootfs and leasing a network are independent —
            // running them concurrently overlaps the (currently dominant)
            // cost of the rootfs copy with the lease instead of paying for
            // both serially.
            let (copy_result, lease_result) = std::thread::scope(|scope| {
                let copy_handle = scope.spawn(|| clone_rootfs(&state.config.base_rootfs_path, &rootfs_path));
                let lease_handle = scope.spawn(|| state.network.lease());
                (copy_handle.join().expect("rootfs copy thread panicked"), lease_handle.join().expect("lease thread panicked"))
            });
            copy_result?;
            let lease = lease_result?;

            let vm = Vm::boot(&VmConfig {
                firecracker_bin: state.config.firecracker_bin.clone(),
                kernel_path: state.config.kernel_path.clone(),
                rootfs_path,
                vcpu_count: state.config.vcpu_count,
                mem_size_mib: state.config.mem_size_mib,
                network: Some(lease.config.clone()),
                extra_drives,
            });
            match vm {
                Ok(vm) => Ok((vm, lease)),
                Err(e) => {
                    let _ = state.network.release(lease);
                    Err(e)
                }
            }
        }
    })
    .await
    .expect("boot task panicked")?;

    let sandbox = Sandbox {
        id: id.clone(),
        vm,
        network,
        rootfs_path,
        attached_drives: attached_drive_ids,
        tags: request.tags,
        created_at: SystemTime::now(),
    };
    state.sandboxes.lock().unwrap().insert(id.clone(), sandbox);

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
    let sandbox = state.sandboxes.lock().unwrap().remove(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;

    tokio::task::spawn_blocking(move || {
        let _ = sandbox.vm.stop();
        let _ = state.network.release(sandbox.network);
        let _ = std::fs::remove_file(&sandbox.rootfs_path);
    })
    .await
    .expect("stop task panicked");

    Ok(StatusCode::NO_CONTENT)
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
