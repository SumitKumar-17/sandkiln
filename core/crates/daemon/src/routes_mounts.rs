//! Remote storage mounts: an S3-compatible object store, mounted into a
//! sandbox and read/written through its normal filesystem interface via
//! FUSE (`rclone mount`, a single statically-linked binary injected into
//! the rootfs the same way the guest agent itself is — see
//! `images/inject-rclone.sh` — rather than an apt package, since the
//! actual test rootfs image's package manager cannot be relied on; see
//! `images/AGENTS.md`). The guest kernel also has to be built with
//! `CONFIG_FUSE_FS` — Firecracker's own default/CI kernel configs don't
//! enable it — see `images/build-guest-kernel.sh`.
//!
//! Firecracker has no `virtio-fs`/`9p` support at all — a real,
//! deliberate upstream constraint, not something this project could work
//! around — so a host-side directory passthrough into the guest was
//! never an option. The only way to give a sandbox an S3-backed
//! filesystem is to run the FUSE client *inside* the guest, which turns
//! out to need no new wire protocol at all: this module orchestrates the
//! whole thing purely with requests `sandkiln-protocol` already has —
//! `Mkdir`, `WriteFile`, `Chmod`, `Exec` — the same primitives
//! `routes_fs.rs`/`routes_exec.rs` already expose directly to callers,
//! just chained together here into one clean daemon-side operation.
//!
//! **Credentials never touch a command-line argument.** rclone (like
//! most S3 clients) reads credentials out of a config file rather than a
//! flag — this module always uses that form, writing a small
//! single-remote config via `WriteFile` and locking it to `0600` via
//! `Chmod` before ever invoking `rclone`, specifically because a
//! command-line argument would be visible to anything running `ps aux`
//! inside the guest. Credentials are written into the guest and then
//! never seen again by the daemon — not carried on `Sandbox`/
//! `Snapshot`/`RetiredSnapshot` (see `state::Mount`'s own doc comment),
//! not logged, not persisted anywhere on the host.
//!
//! **Mounting and unmounting both run as the guest agent's own user
//! (root — see `images/inject-agent.sh`'s systemd unit), so this needs
//! no `fusermount`/`fuse` userspace helper at all.** `rclone mount
//! --daemon` opens `/dev/fuse` and calls `mount(2)` directly, and a plain
//! `umount` tears it back down — the setuid `fusermount` helper only
//! exists to let non-root users do the same, which doesn't apply here.
//!
//! **No re-application on resume/fork/restore, unlike drives/egress/
//! network.** A mount is a live guest-side FUSE *process* — Firecracker's
//! snapshot mechanism already captures a running process's full state
//! along with everything else in guest memory, so a resumed/forked/
//! restored sandbox's mount just keeps working with zero daemon
//! involvement. `Sandbox::mounts`/`Snapshot::mounts`/
//! `RetiredSnapshot::mounts` exist purely so `GET /sandboxes/:id/mounts`
//! has something to list without a live round-trip into the guest.
//!
//! **No holder-tracking/exclusivity, unlike drives.** Two sandboxes
//! mounting the same bucket concurrently doesn't have the shared-mutable-
//! block-device corruption risk a local drive does — S3's own object
//! semantics don't have that hazard — so there's nothing here mirroring
//! `AppState::drive_holders`.

use crate::error::AppError;
use crate::routes_exec::call_agent;
use crate::state::{AppState, Mount};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, post};
use axum::{Json, Router};
use base64::Engine;
use sandkiln_protocol::{Request, Response as AgentResponse};
use serde::{Deserialize, Serialize};
use std::io;
use std::sync::Arc;
use uuid::Uuid;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sandboxes/:id/mounts", post(create_mount).get(list_mounts))
        .route("/sandboxes/:id/mounts/:mount_id", delete(delete_mount))
}

#[derive(Deserialize)]
pub struct CreateMountRequest {
    bucket: String,
    /// Full URL of the S3-compatible endpoint — required, never defaulted
    /// to any particular provider, so a mount's target is always
    /// explicit. See the module doc comment for why this can't be a
    /// caller-supplied credential shortcut either.
    endpoint: String,
    access_key: String,
    secret_key: String,
    /// Where inside the guest this mount is attached — created (`mkdir
    /// -p`) if it doesn't already exist.
    mount_path: String,
    #[serde(default)]
    read_only: bool,
}

#[derive(Serialize, Clone)]
pub struct MountResponse {
    id: String,
    bucket: String,
    endpoint: String,
    mount_path: String,
    read_only: bool,
}

impl From<Mount> for MountResponse {
    fn from(m: Mount) -> Self {
        Self { id: m.id, bucket: m.bucket, endpoint: m.endpoint, mount_path: m.mount_path, read_only: m.read_only }
    }
}

#[derive(Serialize)]
pub struct ListMountsResponse {
    mounts: Vec<MountResponse>,
}

/// Basic input sanity, not a security boundary — this project's existing
/// filesystem-op handlers (`routes_exec`/`routes_fs`) deliberately do no
/// path validation of their own either (see `routes_fs.rs`'s own module
/// doc comment for why that's a consistency choice), and `mount_path`
/// here is no different: it's a path inside the guest's own root-owned
/// space, not a host path.
fn validate_create_mount_request(req: &CreateMountRequest) -> Result<(), String> {
    if req.bucket.is_empty() {
        return Err("bucket must not be empty".to_string());
    }
    if req.endpoint.is_empty() {
        return Err("endpoint must not be empty".to_string());
    }
    if req.access_key.is_empty() {
        return Err("access_key must not be empty".to_string());
    }
    if req.secret_key.is_empty() {
        return Err("secret_key must not be empty".to_string());
    }
    if req.mount_path.is_empty() {
        return Err("mount_path must not be empty".to_string());
    }
    Ok(())
}

/// A minimal single-remote rclone config, named `sandkiln` — each mount
/// gets its own private config file (see `config_path_for`), so there's
/// never a naming collision between mounts to worry about.
/// `provider = Other` + an explicit `endpoint` is what makes a
/// self-hosted S3-compatible store (the common case for this feature,
/// per its own "S3-compatible," not "AWS specifically," framing) reachable
/// at all, and is harmless against a real AWS endpoint too.
const RCLONE_REMOTE_NAME: &str = "sandkiln";

fn rclone_config_content(access_key: &str, secret_key: &str, endpoint: &str) -> String {
    format!(
        "[{RCLONE_REMOTE_NAME}]\n\
         type = s3\n\
         provider = Other\n\
         env_auth = false\n\
         access_key_id = {access_key}\n\
         secret_access_key = {secret_key}\n\
         endpoint = {endpoint}\n"
    )
}

/// `rclone mount`'s own args — `--daemon` so the guest-agent `Exec` call
/// (which waits for the process to exit) returns once the mount is ready
/// rather than blocking forever on a long-running foreground process.
fn rclone_mount_args(config_path: &str, bucket: &str, mount_path: &str, read_only: bool) -> Vec<String> {
    let mut args = vec![
        "mount".to_string(),
        format!("{RCLONE_REMOTE_NAME}:{bucket}"),
        mount_path.to_string(),
        "--config".to_string(),
        config_path.to_string(),
        "--daemon".to_string(),
        "--daemon-wait=30s".to_string(),
    ];
    if read_only {
        args.push("--read-only".to_string());
    }
    args
}

fn config_path_for(mount_id: &str) -> String {
    format!("/root/.sandkiln-rclone-{mount_id}.conf")
}

/// `Mkdir`/`WriteFile`/`Chmod` all report success as a bare
/// `Response::Ok`, not `Response::Exec` — mirrors `routes_fs.rs`'s own
/// private `ok_or_bad_request`, which isn't reusable from here.
fn expect_ok(response: AgentResponse) -> Result<(), AppError> {
    match response {
        AgentResponse::Ok => Ok(()),
        AgentResponse::Error { message } => Err(AppError::BadRequest(message)),
        other => Err(AppError::Internal(io::Error::other(format!("unexpected agent response: {other:?}")))),
    }
}

fn exec_ok(response: AgentResponse) -> Result<(String, String, i32), AppError> {
    match response {
        AgentResponse::Exec { stdout, stderr, exit_code } => Ok((stdout, stderr, exit_code)),
        AgentResponse::Error { message } => Err(AppError::BadRequest(message)),
        other => Err(AppError::Internal(io::Error::other(format!("unexpected agent response: {other:?}")))),
    }
}

/// Mounts a remote S3-compatible bucket into a sandbox. See the module
/// doc comment for the full orchestration and credential-handling design.
#[tracing::instrument(skip(state, request), fields(sandbox_id = %id, bucket = %request.bucket, mount_path = %request.mount_path))]
pub async fn create_mount(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<CreateMountRequest>,
) -> Result<Json<MountResponse>, AppError> {
    validate_create_mount_request(&request).map_err(AppError::BadRequest)?;
    {
        let sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
        if let Some(existing) = sandbox.mounts.iter().find(|m| m.mount_path == request.mount_path) {
            return Err(AppError::Conflict(format!(
                "{} is already mounted at {} — unmount it first (DELETE /sandboxes/{id}/mounts/{})",
                existing.bucket, existing.mount_path, existing.id
            )));
        }
    }

    let mount_id = Uuid::new_v4().to_string();
    let config_path = config_path_for(&mount_id);

    expect_ok(call_agent(state.clone(), id.clone(), Request::Mkdir { path: request.mount_path.clone(), parents: true }).await?)?;

    let config_base64 = base64::engine::general_purpose::STANDARD
        .encode(rclone_config_content(&request.access_key, &request.secret_key, &request.endpoint));
    expect_ok(
        call_agent(state.clone(), id.clone(), Request::WriteFile { path: config_path.clone(), content_base64: config_base64 }).await?,
    )?;
    expect_ok(call_agent(state.clone(), id.clone(), Request::Chmod { path: config_path.clone(), mode: 0o600 }).await?)?;

    let args = rclone_mount_args(&config_path, &request.bucket, &request.mount_path, request.read_only);
    let mount_response =
        call_agent(state.clone(), id.clone(), Request::Exec { command: "rclone".to_string(), args }).await?;
    let (stdout, stderr, exit_code) = exec_ok(mount_response)?;
    if exit_code != 0 {
        let _ =
            call_agent(state.clone(), id.clone(), Request::Exec { command: "rm".to_string(), args: vec!["-f".to_string(), config_path] })
                .await;
        return Err(AppError::BadRequest(format!("rclone mount failed (exit {exit_code}): {stderr}{stdout}")));
    }

    // Real verification, not just a trusted exit code -- matches this
    // project's existing "verify, don't assume" standard (e.g. a pool
    // claim's own post-resume health check).
    let verify = call_agent(
        state.clone(),
        id.clone(),
        Request::Exec { command: "mountpoint".to_string(), args: vec!["-q".to_string(), request.mount_path.clone()] },
    )
    .await?;
    if !matches!(verify, AgentResponse::Exec { exit_code: 0, .. }) {
        let _ = call_agent(
            state.clone(),
            id.clone(),
            Request::Exec { command: "umount".to_string(), args: vec![request.mount_path.clone()] },
        )
        .await;
        let _ =
            call_agent(state.clone(), id.clone(), Request::Exec { command: "rm".to_string(), args: vec!["-f".to_string(), config_path] })
                .await;
        return Err(AppError::Internal(io::Error::other(
            "rclone reported success but the mount point isn't actually mounted",
        )));
    }

    let mount = Mount {
        id: mount_id,
        bucket: request.bucket,
        endpoint: request.endpoint,
        mount_path: request.mount_path,
        read_only: request.read_only,
    };
    {
        let mut sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get_mut(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
        sandbox.mounts.push(mount.clone());
    }

    Ok(Json(mount.into()))
}

pub async fn list_mounts(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<ListMountsResponse>, AppError> {
    let sandboxes = state.sandboxes.lock().unwrap();
    let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
    let mounts = sandbox.mounts.iter().cloned().map(MountResponse::from).collect();
    Ok(Json(ListMountsResponse { mounts }))
}

/// Unmounts and stops tracking a remote storage mount. Removed from
/// `Sandbox::mounts` regardless of whether the guest-side `umount`
/// call itself succeeds — a stuck FUSE mount (e.g. "device busy") is a
/// loud warning, not a failed request, the same "best-effort past this
/// point" treatment this project already gives other guest-side cleanup
/// that a caller can't retry into working (see `routes_sandbox::
/// destroy_unhealthy_claim`'s neighboring reasoning for `force_stop`).
#[tracing::instrument(skip(state), fields(sandbox_id = %id, mount_id = %mount_id))]
pub async fn delete_mount(
    State(state): State<Arc<AppState>>,
    Path((id, mount_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let mount = {
        let mut sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get_mut(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
        let idx = sandbox.mounts.iter().position(|m| m.id == mount_id).ok_or_else(|| AppError::NotFound(mount_id.clone()))?;
        sandbox.mounts.remove(idx)
    };

    let unmount_result =
        call_agent(state.clone(), id.clone(), Request::Exec { command: "umount".to_string(), args: vec![mount.mount_path.clone()] })
            .await;
    let _ = call_agent(
        state.clone(),
        id.clone(),
        Request::Exec { command: "rm".to_string(), args: vec!["-f".to_string(), config_path_for(&mount.id)] },
    )
    .await;

    match unmount_result {
        Ok(AgentResponse::Exec { exit_code: 0, .. }) => {}
        Ok(AgentResponse::Exec { exit_code, stderr, stdout }) => {
            tracing::warn!(exit_code, %stderr, %stdout, "umount reported a non-zero exit unmounting a remote storage mount");
        }
        Ok(other) => {
            tracing::warn!(response = ?other, "unexpected agent response unmounting a remote storage mount");
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to unmount a remote storage mount cleanly");
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rclone_config_content_includes_credentials_and_endpoint() {
        let cfg = rclone_config_content("AKIAABC", "s3cr3t", "https://s3.example.com");
        assert!(cfg.contains("[sandkiln]"));
        assert!(cfg.contains("type = s3"));
        assert!(cfg.contains("provider = Other"));
        assert!(cfg.contains("access_key_id = AKIAABC"));
        assert!(cfg.contains("secret_access_key = s3cr3t"));
        assert!(cfg.contains("endpoint = https://s3.example.com"));
    }

    #[test]
    fn rclone_mount_args_reference_the_config_and_bucket() {
        let args = rclone_mount_args("/root/.cfg", "my-bucket", "/mnt/data", false);
        assert_eq!(args, vec!["mount", "sandkiln:my-bucket", "/mnt/data", "--config", "/root/.cfg", "--daemon", "--daemon-wait=30s"]);
    }

    #[test]
    fn rclone_mount_args_appends_read_only_flag_when_read_only() {
        let args = rclone_mount_args("/root/.cfg", "my-bucket", "/mnt/data", true);
        assert!(args.contains(&"--read-only".to_string()), "args were: {args:?}");
    }

    #[test]
    fn config_path_for_is_deterministic_and_namespaced_by_mount_id() {
        assert_eq!(config_path_for("abc-123"), "/root/.sandkiln-rclone-abc-123.conf");
    }

    fn sample_request() -> CreateMountRequest {
        CreateMountRequest {
            bucket: "my-bucket".to_string(),
            endpoint: "https://s3.example.com".to_string(),
            access_key: "AKIAABC".to_string(),
            secret_key: "s3cr3t".to_string(),
            mount_path: "/mnt/data".to_string(),
            read_only: false,
        }
    }

    #[test]
    fn validate_create_mount_request_accepts_a_complete_request() {
        assert!(validate_create_mount_request(&sample_request()).is_ok());
    }

    #[test]
    fn validate_create_mount_request_rejects_each_empty_field() {
        let mut req = sample_request();
        req.bucket = String::new();
        assert!(validate_create_mount_request(&req).is_err());

        let mut req = sample_request();
        req.endpoint = String::new();
        assert!(validate_create_mount_request(&req).is_err());

        let mut req = sample_request();
        req.access_key = String::new();
        assert!(validate_create_mount_request(&req).is_err());

        let mut req = sample_request();
        req.secret_key = String::new();
        assert!(validate_create_mount_request(&req).is_err());

        let mut req = sample_request();
        req.mount_path = String::new();
        assert!(validate_create_mount_request(&req).is_err());
    }
}
